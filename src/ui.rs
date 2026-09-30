use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::collections::VecDeque;

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints, Legend, PlotBounds, GridMark};

use zenoh::Config;
use crate::topic_node::{TopicNode, parse_numeric};

const MAX_MESSAGES_PER_TOPIC: usize = 1000;
const OSCILLOSCOPE_TIME_WINDOW: f64 = 10.0;
const UPDATE_INTERVAL_MS: u64 = 33; // ~30 FPS

const MIN_SCOPE_HEIGHT_PX: f32 = 300.0;
const MIN_SCOPE_WIDTH_PX: f32 = 380.0;

const MIN_COMBINED_SCOPE_HEIGHT_PX: f32 = MIN_SCOPE_HEIGHT_PX * 1.2;

const MAX_VISIBLE_ROWS: usize = 5; // includes combined scope row

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
    /// Topics shown together in the combined (overlay) scope.
    combined_scope_topics: Vec<String>,
    available_topics: Vec<String>,
    oscilloscope_data: Arc<Mutex<OscilloscopeData>>,
    frame_count: u64,
    maximized_sent: bool,
    last_screen_width: f32,
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

                                    // Try to parse payload data to accept booleans as numeric
                                    if let Some(value) = parse_numeric(&payload) {
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
            combined_scope_topics: Vec::new(),
            available_topics: Vec::new(),
            oscilloscope_data,
            frame_count: 0,
            maximized_sent: false,
            last_screen_width: 0.0, // 0.0 forces update on first frame
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
        if !self.maximized_sent {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            self.maximized_sent = true;
        }

        // ── Responsive scaling based on actual window width ──────────────
        // Recalculates whenever the window is resized or moved to another monitor.
        let screen_width = ctx.input(|i| i.screen_rect().width());
        if (screen_width - self.last_screen_width).abs() > 1.0 {
            // Map window width to a pixels_per_point that keeps UI proportional.
            // These breakpoints cover common resolutions:
            //   < 1280px wide  → compact (0.7)
            //   1280–1920px    → medium  (0.85)
            //   1920–2560px    → large   (1.0)
            //   > 2560px       → scale up proportionally
            let ppp = if screen_width < 1280.0 {
                0.7
            } else if screen_width < 1920.0 {
                // Linear interpolation between 0.7 and 0.85
                0.7 + (screen_width - 1280.0) / (1920.0 - 1280.0) * (0.85 - 0.7)
            } else if screen_width < 2560.0 {
                // Linear interpolation between 0.85 and 1.0
                0.85 + (screen_width - 1920.0) / (2560.0 - 1920.0) * (1.0 - 0.85)
            } else {
                // Beyond 1440p: scale proportionally
                (screen_width / 2560.0).clamp(1.0, 1.5)
            };

            ctx.set_pixels_per_point(ppp);
            self.last_screen_width = screen_width;
        }

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
                // ── Side panel: topic selector ──────────────────────────────
                egui::SidePanel::left("topic_selector")
                    .default_width(260.0)
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
                                    .and_then(|d| d.topic_data.get(topic))
                                    .map(|d| !d.is_empty())
                                    .unwrap_or(false);

                                let mut is_selected = self.selected_topics.contains(topic);
                                let in_combined = self.combined_scope_topics.contains(topic);

                                ui.horizontal(|ui| {
                                    if has_numeric_data {
                                        // Individual scope checkbox
                                        if ui.checkbox(&mut is_selected, "").changed() {
                                            if is_selected {
                                                self.selected_topics.push(topic.clone());
                                            } else {
                                                self.selected_topics.retain(|t| t != topic);
                                            }
                                        }

                                        // "➕" button adds to combined scope
                                        let combined_label = if in_combined { "✖" } else { "➕" };
                                        let combined_tooltip = if in_combined {
                                            "Remove from combined scope"
                                        } else {
                                            "Add to combined scope"
                                        };
                                        if ui.button(combined_label)
                                            .on_hover_text(combined_tooltip)
                                            .clicked()
                                        {
                                            if in_combined {
                                                self.combined_scope_topics.retain(|t| t != topic);
                                            } else {
                                                self.combined_scope_topics.push(topic.clone());
                                            }
                                        }

                                        ui.label(topic);
                                    } else {
                                        ui.add_enabled(false, egui::Checkbox::new(&mut false, ""));
                                        ui.add_enabled(false, egui::Button::new("➕"));
                                        ui.label(
                                            egui::RichText::new(format!("{} (non-numeric)", topic))
                                                .color(egui::Color32::GRAY)
                                        );
                                    }
                                });
                            }
                        });

                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Clear Selection").clicked() {
                                self.selected_topics.clear();
                            }
                            if ui.button("Clear Combined").clicked() {
                                self.combined_scope_topics.clear();
                            }
                        });
                    });

                // ── Central panel: combined scope + individual grid ──────────
                egui::CentralPanel::default().show(ctx, |ui| {
                    let has_combined = !self.combined_scope_topics.is_empty();
                    let has_individual = !self.selected_topics.is_empty();

                    if !has_combined && !has_individual {
                        ui.centered_and_justified(|ui| {
                            ui.label("Select topics from the left panel to visualize.\nUse ➕ to add signals to the combined scope.");
                        });
                        return;
                    }

                    // Combined scope occupies 1 row when present
                    let combined_rows_used: usize = if has_combined { 1 } else { 0 };
                    let max_individual_rows = MAX_VISIBLE_ROWS.saturating_sub(combined_rows_used);

                    let combined_scope_height = if has_combined {
                        MIN_COMBINED_SCOPE_HEIGHT_PX + 40.0 // plot + label + separator
                    } else {
                        0.0
                    };

                    // Compute individual grid dimensions to cap visible height
                    let num_cols = if has_individual {
                        compute_columns(ui.available_width(), self.selected_topics.len())
                    } else {
                        1
                    };
                    let num_rows = if has_individual {
                        (self.selected_topics.len() + num_cols - 1) / num_cols
                    } else {
                        0
                    };
                    let visible_rows = num_rows.min(max_individual_rows);
                    let individual_visible_height = visible_rows as f32 * (MIN_SCOPE_HEIGHT_PX + ui.spacing().item_spacing.y);

                    // Total visible area before scroll
                    let max_visible_height = combined_scope_height + individual_visible_height;

                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .max_height(max_visible_height) // <── caps visible area, enables scroll beyond
                        .show(ui, |ui| {
                            if has_combined {
                                draw_combined_scope(
                                    ui,
                                    &self.oscilloscope_data,
                                    &self.combined_scope_topics,
                                );
                                ui.separator();
                            }

                            if has_individual {
                                draw_oscilloscope_grid(
                                    ui,
                                    &self.oscilloscope_data,
                                    &self.selected_topics,
                                );
                            }
                        });
                });
            }
        }
    }
}

// ── Combined scope: all selected signals overlaid on one plot ────────────────

fn draw_combined_scope(
    ui: &mut egui::Ui,
    oscilloscope_data: &Arc<Mutex<OscilloscopeData>>,
    topics: &[String],
) {
    let osc_data = match oscilloscope_data.lock() {
        Ok(d) => d,
        Err(_) => {
            ui.label("Error: Unable to access oscilloscope data");
            return;
        }
    };

    let current_time = osc_data.get_current_time();
    let window_start = (current_time - OSCILLOSCOPE_TIME_WINDOW).max(0.0);

    let colors = scope_colors();

    ui.label(
        egui::RichText::new("🔀 Combined Scope")
            .strong()
            .size(14.0),
    );

    let available_width = ui.available_width();

    // Compute global Y range across all combined topics
    let all_data: Vec<Vec<[f64; 2]>> = topics
        .iter()
        .map(|t| osc_data.get_data_in_window(t))
        .collect();

    let (y_min, y_max) = {
        let merged: Vec<[f64; 2]> = all_data.iter().flatten().copied().collect();
        compute_y_range(&merged)
    };

    Plot::new("combined_scope")
        .height(MIN_COMBINED_SCOPE_HEIGHT_PX)
        .width(available_width)
        .legend(Legend::default().text_style(egui::TextStyle::Heading))
        .show_axes([true, true])
        .show_grid([true, true])
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .include_x(window_start)
        .include_x(current_time)
        .include_y(y_min)
        .include_y(y_max)
        // ── Force 1-second grid marks on X ──────────────────────────
        .x_grid_spacer(|input| {
            let mut marks = vec![];
            // Use the actual visible bounds, not the full data range
            let start = input.bounds.0.ceil() as i64;   // ceil: first mark INSIDE left edge
            let end = input.bounds.1.floor() as i64;    // floor: last mark INSIDE right edge
            let range = end - start;
            let step = if range > 15 { 2i64 } else { 1i64 };
            let mut t = start;
            while t <= end {
                marks.push(GridMark { value: t as f64, step_size: step as f64 });
                t += step;
            }
            marks
        })
        .x_axis_formatter(|mark, _bounds| {
            format!("{:.0}", mark.value)
        })
        .show(ui, |plot_ui| {
            plot_ui.set_plot_bounds(PlotBounds::from_min_max(
                [window_start, y_min],
                [current_time, y_max],
            ));

            for (idx, (topic, data)) in topics.iter().zip(all_data.iter()).enumerate() {
                if !data.is_empty() {
                    let color = colors[idx % colors.len()];
                    let points: PlotPoints = data.clone().into();
                    plot_ui.line(
                        Line::new(points)
                            .color(color)
                            .name(topic)
                            .width(2.0),
                    );
                }
            }
        });
}

// ── Individual scopes grid ───────────────────────────────────────────────────

fn compute_columns(available_width: f32, scope_count: usize) -> usize {
    let max_cols_by_width = ((available_width / MIN_SCOPE_WIDTH_PX) as usize).max(1);
    let preferred = match scope_count {
        1 => 1,
        2 => 2,
        3..=4 => 2,
        _ => 3,
    };
    preferred.min(max_cols_by_width).min(scope_count)
}

fn scope_colors() -> [egui::Color32; 8] {
    [
        egui::Color32::from_rgb(255, 100, 100),
        egui::Color32::from_rgb(100, 255, 100),
        egui::Color32::from_rgb(100, 100, 255),
        egui::Color32::from_rgb(255, 255, 100),
        egui::Color32::from_rgb(255, 100, 255),
        egui::Color32::from_rgb(100, 255, 255),
        egui::Color32::from_rgb(255, 150, 100),
        egui::Color32::from_rgb(150, 100, 255),
    ]
}

fn draw_oscilloscope_grid(
    ui: &mut egui::Ui,
    oscilloscope_data: &Arc<Mutex<OscilloscopeData>>,
    selected_topics: &[String],
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
    let colors = scope_colors();

    let available_width = ui.available_width();
    let num_cols = compute_columns(available_width, selected_topics.len());
    let scope_width = (available_width / num_cols as f32) - ui.spacing().item_spacing.x;
    // Height: fill available height divided by rows, but respect minimum
    let available_height = ui.available_height();
    let num_rows = (selected_topics.len() + num_cols - 1) / num_cols;
    let scope_height = ((available_height / num_rows as f32) - ui.spacing().item_spacing.y)
        .max(MIN_SCOPE_HEIGHT_PX);

    // Total content height for scroll area
    let total_height = scope_height * num_rows as f32
        + ui.spacing().item_spacing.y * (num_rows as f32 - 1.0);

    for (row_idx, chunk) in selected_topics.chunks(num_cols).enumerate() {
        ui.horizontal(|ui| {
            for (col_idx, topic) in chunk.iter().enumerate() {
                let global_idx = row_idx * num_cols + col_idx;
                let color = colors[global_idx % colors.len()];
                let data = osc_data.get_data_in_window(topic);
                let (y_min, y_max) = compute_y_range(&data);
                let plot_id = format!("scope_{}_{}", row_idx, col_idx);
                let axis_font = egui::FontId::new(
                    if scope_width < 450.0 { 9.0 } else { 11.0 },
                    egui::FontFamily::Proportional,
                );

                ui.allocate_ui(egui::vec2(scope_width, scope_height), |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(topic)
                                .color(color)
                                .size(14.0)
                                .strong(),
                        );

                        Plot::new(&plot_id)
                            .height(scope_height - 20.0)
                            .width(scope_width)
                            .legend(Legend::default().text_style(egui::TextStyle::Heading))
                            .show_axes([true, true])
                            .show_grid([true, true])
                            .allow_zoom(false)
                            .allow_drag(false)
                            .allow_scroll(false)
                            .include_x(window_start)
                            .include_x(current_time)
                            .include_y(y_min)
                            .include_y(y_max)
                            // ── Force 1-second grid marks on X ────────────────────────
                            .x_grid_spacer(|input| {
                                let mut marks = vec![];
                                // Use the actual visible bounds, not the full data range
                                let start = input.bounds.0.ceil() as i64;   // ceil: first mark INSIDE left edge
                                let end = input.bounds.1.floor() as i64;    // floor: last mark INSIDE right edge
                                let range = end - start;
                                let step = if range > 15 { 2i64 } else { 1i64 };
                                let mut t = start;
                                while t <= end {
                                    marks.push(GridMark { value: t as f64, step_size: step as f64 });
                                    t += step;
                                }
                                marks
                            })
                            .x_axis_formatter(|mark, _| format!("{:.0}", mark.value))
                            // ── Smaller axis label font on narrow scopes ──────────────
                            .x_axis_label_style(egui::TextStyle::Name(
                                if scope_width < 450.0 { "axis_small" } else { "axis_normal" }.into()
                            ))
                            .show(ui, |plot_ui| {
                                plot_ui.set_plot_bounds(PlotBounds::from_min_max(
                                    [window_start, y_min],
                                    [current_time, y_max],
                                ));

                                if !data.is_empty() {
                                    let points: PlotPoints = data.into();
                                    plot_ui.line(
                                        Line::new(points)
                                            .color(color)
                                            .name(topic)
                                            .width(2.0),
                                    );
                                }
                            });
                    });
                });
            }
        });
    }

    // Ensure scroll area has enough space
    let used = ui.min_rect().height();
    if used < total_height {
        ui.add_space(total_height - used);
    }
}

/// Compute Y axis range with margin, auto-adjusted to data
fn compute_y_range(data: &[[f64; 2]]) -> (f64, f64) {
    if data.is_empty() {
        return (-1.0, 1.0);
    }

    let y_min = data.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let y_max = data.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);

    if (y_max - y_min).abs() < 1e-10 {
        return (y_min - 1.0, y_max + 1.0);
    }

    let margin = (y_max - y_min) * 0.1;
    (y_min - margin, y_max + margin)
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
            egui::CollapsingHeader::new(format!("📁 {}", node.name))
                .id_salt(&current_path)
                .default_open(indent < 2)
                .show(ui, |ui| {
                    for child in node.children.values() {
                        draw_tree_with_path(ui, child, indent + 1, filter, &current_path);
                    }
                });
        } else if !node.messages.is_empty() {
            let msg_count = node.messages.len();
            let latest_msg = node.messages.last().map(|(_, m)| m.as_str()).unwrap_or("");
            let header_text = format!(
                "📄 {} ({}) = {}",
                node.name,
                msg_count,
                if latest_msg.len() > 20 {
                    format!("{}...", &latest_msg[..20])
                } else {
                    latest_msg.to_string()
                }
            );

            egui::CollapsingHeader::new(header_text)
                .id_salt(&current_path)
                .default_open(false)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .min_scrolled_height(5.0 * 20.0)
                        .max_height(400.0)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            for (ts, msg) in node.messages.iter().rev().take(100) {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(ts.format("%H:%M:%S%.3f").to_string())
                                            .monospace()
                                            .small()
                                            .color(egui::Color32::GRAY),
                                    );
                                    ui.label(egui::RichText::new(msg).monospace().small());
                                });
                            }
                        });
                });
        }
    });
}

fn matches_filter_recursive(node: &TopicNode, filter: &str) -> bool {
    if node.name.to_lowercase().contains(&filter.to_lowercase()) {
        return true;
    }
    node.children.values().any(|child| matches_filter_recursive(child, filter))
}
