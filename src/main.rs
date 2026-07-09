#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// CPU 純粋部(sim / brush / paper / replay モデル)は paint-core crate、
// 顔料・混色は pigment crate、Kubelka-Munk 参照実装は km crate へ切り出した(refactoring.md R1)。
mod app;
mod assets;
mod gpu;
mod input;
mod palette_store;
mod pigment_store;
mod preset;
mod replay;
mod work;

use eframe::egui;

fn main() -> eframe::Result {
    env_logger::init();

    // 配布ビルド(embed-assets)の初回起動時、既定の顔料・パレット・プリセット等を
    // exe 隣へ書き出す(通常ビルドでは何もしない)。ストア読込より前に呼ぶこと
    assets::seed_default_assets();

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("かさねいろ")
            .with_inner_size([1100.0, 780.0]),
        ..Default::default()
    };
    eframe::run_native(
        "kasaneiro",
        options,
        Box::new(|cc| Ok(Box::new(app::PaintApp::new(cc)))),
    )
}
