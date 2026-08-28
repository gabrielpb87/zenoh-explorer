use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::collections::VecDeque;

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints, Legend, PlotBounds};
use zenoh::Config;
use crate::topic_node::TopicNode;

const MAX_MESSAGES_PER_TOPIC: usize = 1000;
const OSCILLOSCOPE_TIME_WINDOW: f64 = 10.0;
const UPDATE_INTERVAL_MS: u64 = 33; // ~30 FPS

#[derive(PartialEq, Clone, Copy)]
enum ViewMode {
    TreeView,
    OscilloscopeView,
}

pub struct App {
    topic_root: Arc<Mutex<TopicNode>>,
    search_filter: String,
    view_mode: ViewMode,
    selected_topics: Vec<String>,
    available_topics: Vec<String>,
    oscilloscope_data: Arc<Mutex<OscilloscopeData>>,
    frame_count: u64,
}

#[derive(Default)]
struct OscilloscopeData {
    topic_data: std::collections::HashMap<String, VecDeque<(f64, f64)>>,
    start_time: Option<std::time::Instant>,
}

impl OscilloscopeData {
    fn add_sample(&mut self, topic: &str, value: f64) {
        let start = self.start_time.get_or_insert_with(std::time::Instant::now);
        let elapsed = start.elapsed().as_secs_f64();
        
        let data = self.topic_data.entry(topic.to_string()).or_insert_with(VecDeque::new);
        data.push_back((elapsed, value));
        
        while let Some((time, _)) = data.front() {
            if elapsed - time > OSCILLOSCOPE_TIME_WINDOW * 2.0 {
                data.pop_front();
            } else {
                break;
            }
        }
    }
    
    fn get_data_in_window(&self, topic: &str) -> Vec<[f64; 2]> {
        if let Some(start) = self.start_time {
            let current_time = start.elapsed().as_secs_f64();
            let window_start = current_time - OSCILLOSCOPE_TIME_WINDOW;
            
            if let Some(data) = self.topic_data.get(topic) {
                return data.iter()
                    .filter(|(t, _)| *t >= window_start)
                    .map(|(t, v)| [*t, *v])
                    .collect();
            }
        }
        Vec::new()
    }
    
    fn get_current_time(&self) -> f64 {
        self.start_time
            .map(|start| start.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }
}

impl App {
    pub fn new(config_path: Option<&str>) -> Self {
        eprintln!("App::new() called");
        
        let topic_root = Arc::new(Mutex::new(TopicNode {
            name: "root".to_string(),
            ..Default::default()
        }));

        let tree_clone = topic_root.clone();
        let oscilloscope_data = Arc::new(Mutex::new(OscilloscopeData::default()));
        let osc_clone = oscilloscope_data.clone();

        let config = config_path
            .map(|path| {
                eprintln!("Loading config from: {}", path);
                zenoh::Config::from_file(path).unwrap_or_else(|e| {
                    eprintln!("Failed to load config '{}': {}", path, e);
                    Config::default()
                })
            })
            .unwrap_or_else(Config::default);

        eprintln!("Spawning Zenoh thread...");
        thread::spawn(move || {
            eprintln!("Zenoh thread started");
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("Failed to create tokio runtime: {}", e);
                    return;
                }
            };
            
            rt.block_on(async move {
                zenoh::init_log_from_env_or("error");
                
                match zenoh::open(config).await {
                    Ok(session) => {
                        eprintln!("Zenoh session opened");
                        match session.declare_subscriber("**").await {
                            Ok(subscriber) => {
                                eprintln!("Zenoh subscriber created successfully");
                                while let Ok(sample) = subscriber.recv_async().await {
                                    let topic = sample.key_expr().as_str();
                                    let path: Vec<&str> = topic.split('/').collect();
                                    
                                    let payload = sample.payload()
                                        .try_to_string()
                                        .map(|cow| cow.into_owned())
                                        .unwrap_or_else(|e| e.to_string());

                                    if let Ok(value) = payload.trim().parse::<f64>() {
                                        if let Ok(mut osc) = osc_clone.lock() {
                                            osc.add_sample(topic, value);
                                        }
                                    }

                                    if let Ok(mut tree) = tree_clone.lock() {
                                        tree.add_message(&path, payload, MAX_MESSAGES_PER_TOPIC);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("Failed to create subscriber: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to open Zenoh session: {}", e);
                    }
                }
            });
        });

        eprintln!("App::new() completed");
        
        App { 
            topic_root,
            search_filter: String::new(),
            view_mode: ViewMode::TreeView,
            selected_topics: Vec::new(),
            available_topics: Vec::new(),
            oscilloscope_data,
            frame_count: 0,
        }
    }

    fn update_available_topics(&mut self) {
        if let Ok(tree) = self.topic_root.lock() {
            self.available_topics.clear();
            tree.collect_leaf_topics("", &mut self.available_topics);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame_count += 1;
        
        // Continuous repaint
        ctx.request_repaint_after(Duration::from_millis(UPDATE_INTERVAL_MS));

        if self.frame_count % 30 == 0 {
            self.update_available_topics();
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🔍 Zenoh Explorer");
                ui.separator();
                
                ui.label("View:");
                ui.selectable_value(&mut self.view_mode, ViewMode::TreeView, "📁 Tree");
                ui.selectable_value(&mut self.view_mode, ViewMode::OscilloscopeView, "📊 Oscilloscope");
                
                ui.separator();
                
                if self.view_mode == ViewMode::TreeView {
                    ui.label("Filter:");
                    ui.text_edit_singleline(&mut self.search_filter);
                    
                    if ui.button("Clear Filter").clicked() {
                        self.search_filter.clear();
                    }
                }
                
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Clear All").clicked() {
                        if let Ok(mut tree) = self.topic_root.lock() {
                            tree.clear_all_messages();
                        }
                        if let Ok(mut osc) = self.oscilloscope_data.lock() {
                            osc.topic_data.clear();
                            osc.start_time = None;
                        }
                    }
                });
            });
        });

        match self.view_mode {
            ViewMode::TreeView => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            match self.topic_root.lock() {
                                Ok(tree) => draw_tree(ui, &tree, 0, &self.search_filter),
                                Err(_) => {
                                    ui.label("Error: Unable to access topic tree");
                                }
                            }
                        });
                });
            }
            ViewMode::OscilloscopeView => {
                egui::SidePanel::left("topic_selector")
                    .default_width(250.0)
                    .show(ctx, |ui| {
                        ui.heading("Select Topics");
                        ui.separator();
                        
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let topics = self.available_topics.clone();
                            
                            // Get oscilloscope data to check which topics have numeric values
                            let osc_data = self.oscilloscope_data.lock().ok();
                            
                            for topic in &topics {
                                // Check if topic has numeric data
                                let has_numeric_data = osc_data
                                    .as_ref()
                                    .and_then(|data| data.topic_data.get(topic))
                                    .map(|data| !data.is_empty())
                                    .unwrap_or(false);
                                
                                let mut is_selected = self.selected_topics.contains(topic);
                                
                                ui.horizontal(|ui| {
                                    if has_numeric_data {
                                        // Normal checkbox for numeric topics
                                        if ui.checkbox(&mut is_selected, "").changed() {
                                            if is_selected {
                                                self.selected_topics.push(topic.clone());
                                            } else {
                                                self.selected_topics.retain(|t| t != topic);
                                            }
                                        }
                                        ui.label(topic);
                                    } else {
                                        // Disabled checkbox for non-numeric topics
                                        ui.add_enabled(false, egui::Checkbox::new(&mut false, ""));
                                        ui.label(
                                            egui::RichText::new(format!("{} (not numerical value)", topic))
                                                .color(egui::Color32::GRAY)
                                        );
                                    }
                                });
                            }
                        });
                        
                        ui.separator();
                        if ui.button("Clear Selection").clicked() {
                            self.selected_topics.clear();
                        }
                    });

                egui::CentralPanel::default().show(ctx, |ui| {
                    if self.selected_topics.is_empty() {
                        ui.centered_and_justified(|ui| {
                            ui.label("Select topics from the left panel to visualize");
                        });
                    } else {
                        draw_oscilloscope_realtime(ui, &self.oscilloscope_data, &self.selected_topics);
                    }
                });
            }
        }
    }
}

fn draw_tree(ui: &mut egui::Ui, node: &TopicNode, indent: usize, filter: &str) {
    draw_tree_with_path(ui, node, indent, filter, "");
}

fn draw_tree_with_path(ui: &mut egui::Ui, node: &TopicNode, indent: usize, filter: &str, parent_path: &str) {
    let indent_width = 20.0;
    
    if !filter.is_empty() && !node.name.to_lowercase().contains(&filter.to_lowercase()) {
        let has_matching_child = node.children.values()
            .any(|child| matches_filter_recursive(child, filter));
        if !has_matching_child && node.messages.is_empty() {
            return;
        }
    }

    // Build full path for unique ID
    let current_path = if parent_path.is_empty() || parent_path == "root" {
        node.name.clone()
    } else {
        format!("{}/{}", parent_path, node.name)
    };

    ui.horizontal(|ui| {
        ui.add_space(indent as f32 * indent_width);

        if !node.children.is_empty() {
            let header_text = format!("📁 {}", node.name);
            egui::CollapsingHeader::new(header_text)
                .id_source(&current_path)
                .default_open(indent < 2)
                .show(ui, |ui| {
                    for child in node.children.values() {
                        draw_tree_with_path(ui, child, indent + 1, filter, &current_path);
                    }
                });
        } else if !node.messages.is_empty() {
            let msg_count = node.messages.len();
            let latest_msg = node.messages.last().map(|(_, m)| m.as_str()).unwrap_or("");
            let header_text = format!("📄 {} ({}) = {}", node.name, msg_count, 
                if latest_msg.len() > 20 { 
                    format!("{}...", &latest_msg[..20]) 
                } else { 
                    latest_msg.to_string() 
                });
            
            egui::CollapsingHeader::new(header_text)
                .id_source(&current_path)
                .default_open(false)
                .show(ui, |ui| {
                    // Calculate height to show at least 5 messages
                    // Each message line is approximately 20 pixels
                    let min_height = 5.0 * 20.0; // At least 5 messages
                    let max_height = 400.0;      // Maximum scroll area height
                    
                    egui::ScrollArea::vertical()
                        .min_scrolled_height(min_height)
                        .max_height(max_height)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            
                            // Show most recent messages first
                            for (ts, msg) in node.messages.iter().rev().take(100) {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(ts.format("%H:%M:%S%.3f").to_string())
                                            .monospace()
                                            .small()
                                            .color(egui::Color32::GRAY)
                                    );
                                    ui.label(
                                        egui::RichText::new(msg)
                                            .monospace()
                                            .small()
                                    );
                                });
                            }
                        });
                });
        }
    });
}

fn draw_oscilloscope_realtime(
    ui: &mut egui::Ui, 
    oscilloscope_data: &Arc<Mutex<OscilloscopeData>>, 
    selected_topics: &[String]
) {
    let osc_data = match oscilloscope_data.lock() {
        Ok(data) => data,
        Err(_) => {
            ui.label("Error: Unable to access oscilloscope data");
            return;
        }
    };
    
    let current_time = osc_data.get_current_time();
    let window_start = (current_time - OSCILLOSCOPE_TIME_WINDOW).max(0.0);
    
    let colors = [
        egui::Color32::from_rgb(255, 100, 100),
        egui::Color32::from_rgb(100, 255, 100),
        egui::Color32::from_rgb(100, 100, 255),
        egui::Color32::from_rgb(255, 255, 100),
        egui::Color32::from_rgb(255, 100, 255),
        egui::Color32::from_rgb(100, 255, 255),
        egui::Color32::from_rgb(255, 150, 100),
        egui::Color32::from_rgb(150, 100, 255),
    ];

    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    
    for topic in selected_topics {
        let data = osc_data.get_data_in_window(topic);
        for [_, y] in &data {
            y_min = y_min.min(*y);
            y_max = y_max.max(*y);
        }
    }
    
    if !y_min.is_finite() || !y_max.is_finite() || (y_max - y_min).abs() < 1e-10 {
        y_min = -1.0;
        y_max = 1.0;
    } else {
        let margin = (y_max - y_min) * 0.1;
        y_min -= margin;
        y_max += margin;
    }

    Plot::new("oscilloscope_realtime")
        .legend(Legend::default())
        .show_axes([true, true])
        .show_grid([true, true])
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .include_x(window_start)
        .include_x(current_time)
        .include_y(y_min)
        .include_y(y_max)
        .show(ui, |plot_ui| {
            plot_ui.set_plot_bounds(PlotBounds::from_min_max(
                [window_start, y_min],
                [current_time, y_max]
            ));
            
            for (idx, topic) in selected_topics.iter().enumerate() {
                let data = osc_data.get_data_in_window(topic);
                
                if !data.is_empty() {
                    let points: PlotPoints = data.into();
                    let color = colors[idx % colors.len()];
                    
                    plot_ui.line(
                        Line::new(points)
                            .color(color)
                            .name(topic)
                            .width(2.0)
                    );
                }
            }
        });
}

fn matches_filter_recursive(node: &TopicNode, filter: &str) -> bool {
    if node.name.to_lowercase().contains(&filter.to_lowercase()) {
        return true;
    }
    node.children.values().any(|child| matches_filter_recursive(child, filter))
}