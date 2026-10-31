#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// 本体はライブラリ部(lib.rs)。e2e テストも同じものをウィンドウなしで駆動する
use eframe::egui;
use kasaneiro::{app, assets};

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
