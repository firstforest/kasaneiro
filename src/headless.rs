//! ウィンドウなしでキャンバスを駆動する(e2e テスト用)。
//!
//! アプリの「ストローク再生」(H5)と同じ経路をたどる: [`Player`] が記録時のテンポで splat を出し、
//! [`GpuCanvas::encode_frame`] がフレームごとに splat 注入とシミュレーションを積む。
//! 結果は [`GpuCanvas::snapshot`](display.wgsl で焼いた見た目)として読み戻す。
//! wgpu の Device / Queue は呼び出し側が用意する(テストでは egui_kittest の render state を借りる)。

use crate::gpu::hot_reload::shader_dir;
use crate::gpu::{CanvasCallback, GpuCanvas, ViewUniform};
use crate::replay::{Player, Recording};
use eframe::egui_wgpu::wgpu;
use paint_core::sim::{SimParams, Splat};
use pigment::Palette;
use std::path::Path;

/// 表示先フォーマット。スナップショットはこれに合わせて sRGB で焼かれる(画面と同じ見た目)
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// 再生が終わらない記録(壊れたフレーム番号など)でテストが止まらないための上限
const MAX_REPLAY_FRAMES: u32 = 100_000;

pub struct HeadlessCanvas {
    device: wgpu::Device,
    queue: wgpu::Queue,
    canvas: GpuCanvas,
    palette: Palette,
    /// 毎フレーム GPU へ渡すパラメータ。再生中は Player が顔料スロットとツールを上書きする
    pub params: SimParams,
    /// 1フレームあたりのシミュレーションステップ数(アプリの既定と同じ 1)
    pub steps_per_frame: u32,
}

impl HeadlessCanvas {
    /// 既定パレット・既定パラメータの白紙キャンバスを作る。WGSL は assets/shaders/ から読む
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, size: u32) -> Result<Self, String> {
        let mut canvas = GpuCanvas::new(&device, &queue, TARGET_FORMAT, shader_dir(), size);
        canvas.rebuild_pipelines(&device)?;
        let palette = Palette::default_palette();
        canvas.set_palette(&queue, &palette);
        Ok(Self {
            device,
            queue,
            canvas,
            palette,
            params: SimParams::default(),
            steps_per_frame: 1,
        })
    }

    pub fn size(&self) -> u32 {
        self.canvas.size()
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.canvas.set_palette(&self.queue, &palette);
        self.palette = palette;
    }

    /// 1フレーム進める(splat を注入してから steps_per_frame ステップ)
    pub fn frame(&mut self, splats: Vec<Splat>) {
        let frame = CanvasCallback {
            params: self.params,
            splats,
            sim_steps: self.steps_per_frame,
            line_target: None,
            view: ViewUniform::default(),
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("headless_frame"),
            });
        self.canvas.encode_frame(&self.queue, &mut encoder, &frame);
        self.queue.submit([encoder.finish()]);
    }

    /// 入力なしで `frames` フレーム進める(にじみ・乾燥の経過を待つ)
    pub fn idle(&mut self, frames: u32) {
        for _ in 0..frames {
            self.frame(Vec::new());
        }
    }

    /// 記録を最後まで再生する。進めたフレーム数を返す
    pub fn play(&mut self, recording: Recording) -> u32 {
        let mut player = Player::new(recording);
        for n in 1..=MAX_REPLAY_FRAMES {
            let mut splats = Vec::new();
            let more = player.advance(&mut self.params, &mut splats);
            self.frame(splats);
            if !more {
                return n;
            }
        }
        panic!("記録の再生が {MAX_REPLAY_FRAMES} フレームで終わりません");
    }

    /// 「乾かす」: 湿レイヤーを新しい乾燥レイヤーへ焼き込む
    pub fn bake_dry(&mut self) -> Result<(), String> {
        self.canvas
            .bake_dry(&self.device, &self.queue, &self.palette)
    }

    /// Fast Dry: 水だけ除き、浮遊顔料をその場で沈着させる
    pub fn fast_dry(&mut self) -> Result<(), String> {
        self.canvas.fast_dry(&self.device, &self.queue)
    }

    /// 乾燥レイヤーの枚数
    pub fn layer_count(&self) -> usize {
        self.canvas.layers.len()
    }

    /// 現在の見た目を読み戻す
    pub fn snapshot(&self) -> Result<Image, String> {
        let rgba = self.canvas.snapshot(&self.device, &self.queue)?;
        Ok(Image {
            size: self.canvas.size(),
            rgba,
        })
    }
}

/// キャンバスの見た目(sRGB の RGBA8、行連続、size × size)
#[derive(Clone)]
pub struct Image {
    pub size: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// 画素 (x, y) の RGB(0..1、sRGB のまま)
    pub fn rgb(&self, x: u32, y: u32) -> [f32; 3] {
        let i = ((y * self.size + x) * 4) as usize;
        [0, 1, 2].map(|c| self.rgba[i + c] as f32 / 255.0)
    }

    /// 矩形 [x0, x1) × [y0, y1) の平均 RGB(0..1)
    pub fn mean_rgb(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> [f32; 3] {
        let mut sum = [0.0f32; 3];
        let mut n = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = self.rgb(x, y);
                for c in 0..3 {
                    sum[c] += p[c];
                }
                n += 1.0;
            }
        }
        sum.map(|s| s / n)
    }

    /// PNG として書き出す(テスト失敗時に目で確かめる用)
    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
        }
        image::save_buffer(
            path,
            &self.rgba,
            self.size,
            self.size,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("{} に書けません: {e}", path.display()))
    }
}
