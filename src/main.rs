#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// 本体はライブラリ部(lib.rs)。e2e テストも同じものをウィンドウなしで駆動する
use eframe::egui;
use kasaneiro::{app, assets};

fn main() -> eframe::Result {
    // --capture: 解析用に起動直後から終了までを target/capture/<日時>/ へ記録する
    let capture_dir = kasaneiro::capture::dir_from_args();
    kasaneiro::capture::init_logging(capture_dir.as_deref());
    if let Some(dir) = &capture_dir {
        eprintln!("解析用に記録中: {}", dir.display());
    }

    // 配布ビルド(embed-assets)の初回起動時、既定の顔料・パレット・プリセット等を
    // exe 隣へ書き出す(通常ビルドでは何もしない)。ストア読込より前に呼ぶこと
    assets::seed_default_assets();

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_options(),
        viewport: egui::ViewportBuilder::default()
            .with_title("かさねいろ")
            .with_inner_size([1100.0, 780.0]),
        ..Default::default()
    };
    eframe::run_native(
        "kasaneiro",
        options,
        Box::new(|cc| Ok(Box::new(app::PaintApp::with_capture(cc, capture_dir)))),
    )
}

/// eframe 既定の wgpu 設定に、アダプタが対応していれば `CLEAR_TEXTURE` の要求を足す
/// (「全部消す」・線画 Undo のテクスチャゼロ化を GPU 上で行う。gpu/mod.rs の zero_textures)。
/// 非対応のアダプタでは要求せず、ゼロ埋めの write_texture で代替する(起動は妨げない)
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, wgpu};
    let mut config = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        let base = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut desc = base(adapter);
            desc.required_features |= adapter.features() & wgpu::Features::CLEAR_TEXTURE;
            desc
        });
    }
    config
}
