#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod brush;
mod gpu;
mod sim;

use eframe::egui;

fn main() -> eframe::Result {
    env_logger::init();

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("my-paint")
            .with_inner_size([1100.0, 780.0]),
        ..Default::default()
    };
    eframe::run_native(
        "my-paint",
        options,
        Box::new(|cc| Ok(Box::new(app::PaintApp::new(cc)))),
    )
}
