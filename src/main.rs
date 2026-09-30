use eframe::{egui, NativeOptions, egui::ViewportBuilder};
use std::env;
use std::panic;
use std::io::Write;

mod ui;
mod topic_node;

fn main() -> Result<(), eframe::Error> {
    // Fix for WSL: unset WAYLAND_DISPLAY if socket doesn't exist
    if !wayland_socket_exists() {
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
    }

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

    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_min_inner_size(egui::vec2(800.0, 600.0)),
        ..Default::default()
    };

    // let result = eframe::run_native(
    //     "Zenoh Explorer",
    //     options,
    //     Box::new(move |cc| {
    //         cc.egui_ctx.set_visuals(egui::Visuals::dark());

    //         // ── Dynamic DPI scaling based on actual screen size ──────────
    //         // Read the native pixels_per_point reported by the OS/driver
    //         let native_ppp = cc.egui_ctx.pixels_per_point();

    //         // Query the monitor's physical pixel dimensions via the viewport
    //         let screen_size_px = cc.egui_ctx.input(|i| i.screen_rect());

    //         // Logical screen width in egui points at native DPI
    //         let logical_width = screen_size_px.width();

    //         // Target logical width: we want the UI to look as if designed for
    //         // a 1920-wide logical canvas, regardless of physical resolution.
    //         let target_logical_width = 1920.0_f32;

    //         // Compute the scale factor needed to fit our target into actual screen
    //         let scale = (logical_width / target_logical_width).clamp(0.5, 2.0);

    //         // Apply: multiply native ppp by our scale to get consistent sizing
    //         cc.egui_ctx.set_pixels_per_point(native_ppp * scale);

    //         Ok(Box::new(crate::ui::App::new(config_path.as_deref())) as Box<dyn eframe::App>)
    //     })
    // );

    let result = eframe::run_native(
        "Zenoh Explorer",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(crate::ui::App::new(config_path.as_deref())) as Box<dyn eframe::App>)
        })
    );

    eprintln!("eframe exited with: {:?}", result);
    result
}

fn wayland_socket_exists() -> bool {
    if let Ok(display) = std::env::var("WAYLAND_DISPLAY") {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| "/run/user/1000".to_string());
        std::path::Path::new(&format!("{}/{}", runtime_dir, display)).exists()
    } else {
        false
    }
}
