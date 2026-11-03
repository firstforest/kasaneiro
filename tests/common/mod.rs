//! e2e テストの共通部品: テスト用 GPU の用意、記録の組み立て、色の指標。
#![allow(dead_code)] // テストファイルごとに使う部品が違う

use eframe::egui::mutex::RwLock;
use eframe::egui_wgpu;
use kasaneiro::headless::{HeadlessCanvas, Image};
use kasaneiro::replay::{RecordedStroke, Recording, TimedPoint};
use paint_core::tool::WetTool;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// テスト用の wgpu render state(ウィンドウなし)。
///
/// Adapter・Device・Queue はテストのプロセス全体で1つを使い回し、egui の Renderer だけ呼ぶたびに作る
/// (`PaintApp` が Renderer の callback_resources に自分の GpuCanvas を入れるので、並列のテスト間で
/// 共有できない)。テストごとに Device を作ると、並列実行で「あるテストの Device 作成」と「終えた
/// 別テストのスレッド終了」が重なったとき NVIDIA の Vulkan ドライバの中でデッドロックし、
/// テストのプロセスが止まる(ドライバのロックと OS のローダーロックを逆順で取り合う)。
pub fn render_state() -> egui_wgpu::RenderState {
    static SHARED: OnceLock<egui_wgpu::RenderState> = OnceLock::new();
    let shared = SHARED.get_or_init(create_render_state);
    let renderer = egui_wgpu::Renderer::new(
        &shared.device,
        shared.target_format,
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    egui_wgpu::RenderState {
        renderer: Arc::new(RwLock::new(renderer)),
        ..shared.clone()
    }
}

/// egui_kittest の既定セットアップは CPU アダプタ(Windows では DX12 WARP)を優先するが、
/// 流体シミュレーションを CPU で回すと e2e が数十倍遅くなるので実 GPU を選ぶ。
/// `KASANEIRO_TEST_ADAPTER=cpu` で既定(ソフトウェアラスタライザ優先)に戻せる
fn create_render_state() -> egui_wgpu::RenderState {
    let mut setup = egui_kittest::wgpu::default_wgpu_setup();
    let use_cpu = std::env::var("KASANEIRO_TEST_ADAPTER").is_ok_and(|v| v == "cpu");
    if let egui_wgpu::WgpuSetup::CreateNew(create) = &mut setup
        && !use_cpu
    {
        create.native_adapter_selector = Some(std::sync::Arc::new(|adapters, _surface| {
            adapters
                .iter()
                .min_by_key(|a| match a.get_info().device_type {
                    egui_wgpu::wgpu::DeviceType::DiscreteGpu => 0,
                    egui_wgpu::wgpu::DeviceType::IntegratedGpu => 1,
                    egui_wgpu::wgpu::DeviceType::VirtualGpu => 2,
                    egui_wgpu::wgpu::DeviceType::Other => 3,
                    egui_wgpu::wgpu::DeviceType::Cpu => 4,
                })
                .cloned()
                .ok_or_else(|| "wgpu アダプタが見つからない".to_owned())
        }));
    }
    egui_kittest::wgpu::create_render_state(setup, egui_wgpu::RendererOptions::PREDICTABLE)
}

/// 既定パレット・既定パラメータの 512² 白紙キャンバス
pub fn canvas() -> HeadlessCanvas {
    let rs = render_state();
    HeadlessCanvas::new(rs.device, rs.queue, 512).expect("シェーダーのビルドに失敗")
}

/// 結果画像の書き出し先(target/e2e/<name>.png)。指標が外れたとき目で確かめる用
pub fn save(image: &Image, name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("e2e")
        .join(format!("{name}.png"));
    image.save_png(&path).expect("PNG 書き出し");
    path
}

/// (x0, y) から (x1, y) へ水平に引く1ストローク([`stroke`] と同じテンポ)
pub fn hstroke(
    tool: WetTool,
    channel: u32,
    x0: f32,
    x1: f32,
    y: f32,
    start_frame: u32,
) -> RecordedStroke {
    stroke(tool, channel, [x0, y], [x1, y], start_frame)
}

/// a から b へ直線に引く1ストローク(2フレームごとに 10.8 テクセル = 記録済みストロークと同じテンポ)
pub fn stroke(
    tool: WetTool,
    channel: u32,
    a: [f32; 2],
    b: [f32; 2],
    start_frame: u32,
) -> RecordedStroke {
    const STEP: f32 = 10.8;
    let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    let n = (len / STEP).ceil().max(1.0) as u32;
    let points = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            TimedPoint {
                frame: start_frame + i * 2,
                pos: [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
                pressure: 1.0,
            }
        })
        .collect();
    RecordedStroke {
        channel,
        tool: tool.gpu_id(),
        load: None,
        points,
    }
}

pub fn recording(strokes: Vec<RecordedStroke>) -> Recording {
    Recording { strokes }
}

/// sRGB → (色相 0..360°, 彩度 0..1, 明度 0..1)。HSV で十分(色名の帯で判定するため)
pub fn hsv(rgb: [f32; 3]) -> (f32, f32, f32) {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    (h, s, max)
}

/// 2色の差(RGB ユークリッド距離、0..√3)
pub fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// 明るさ(RGB 平均)
pub fn lightness(rgb: [f32; 3]) -> f32 {
    (rgb[0] + rgb[1] + rgb[2]) / 3.0
}
