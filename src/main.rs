#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
mod app;
mod engine;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Fast Copy")
            .with_inner_size([760.0, 580.0])
            .with_min_inner_size([620.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Fast Copy",
        native_options,
        Box::new(|cc| Ok(Box::new(app::FastCopyApp::new(cc)))),
    )
}
