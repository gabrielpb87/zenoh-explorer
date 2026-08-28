use eframe::{egui, NativeOptions, egui::ViewportBuilder};
use std::env;
use std::panic;
use std::io::Write;

mod ui;
mod topic_node;

fn main() -> Result<(), eframe::Error> {
    // Set up panic handler with backtrace
    panic::set_hook(Box::new(|panic_info| {
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "\n========== PANIC ==========");
        if let Some(location) = panic_info.location() {
            let _ = writeln!(stderr, "Location: {}:{}", location.file(), location.line());
        }
        if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
            let _ = writeln!(stderr, "Message: {}", message);
        } else if let Some(message) = panic_info.payload().downcast_ref::<String>() {
            let _ = writeln!(stderr, "Message: {}", message);
        }
        let _ = writeln!(stderr, "===========================\n");
        let _ = stderr.flush();
    }));

    eprintln!("Starting Zenoh Explorer...");

    let args: Vec<String> = env::args().collect();
    let config_path = args.get(1).cloned();

    eprintln!("Creating window options...");
    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size(egui::vec2(1200.0, 800.0))
            .with_min_inner_size(egui::vec2(800.0, 600.0)),
        ..Default::default()
    };

    eprintln!("Initializing eframe...");
    let result = eframe::run_native(
        "Zenoh Explorer",
        options,
        Box::new(move |cc| {
            eprintln!("Creating App instance...");
            // Enable better rendering
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(crate::ui::App::new(config_path.as_deref())) as Box<dyn eframe::App>)
        })
    );

    eprintln!("eframe exited with: {:?}", result);
    result
}