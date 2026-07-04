//! egui の画面構成: パラメータパネル(H2)、キャンバス、シェーダーエラーのオーバーレイ(H1)。

use crate::brush::StrokeState;
use crate::gpu::hot_reload::{ShaderWatcher, shader_dir};
use crate::gpu::{CanvasCallback, GpuCanvas};
use crate::sim::{CANVAS_SIZE, SimParams, Splat};
use eframe::egui;
use eframe::egui_wgpu;

/// egui のデフォルトフォントは日本語グリフを含まないため、
/// Windows のシステムフォントをフォールバックとして追加する(バンドル不要)。
fn install_japanese_font(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];
    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "japanese".to_owned(),
            egui::FontData::from_owned(bytes).into(),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("japanese".to_owned());
        }
        ctx.set_fonts(fonts);
        return;
    }
    log::warn!("日本語フォントが見つかりませんでした。UI の日本語は豆腐表示になります");
}

pub struct PaintApp {
    render_state: egui_wgpu::RenderState,
    params: SimParams,
    brush_color: egui::Color32,
    stroke: StrokeState,
    watcher: ShaderWatcher,
    /// 直近のシェーダービルドエラー(H1: 落とさずオーバーレイ表示)
    shader_error: Option<String>,
}

impl PaintApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_japanese_font(&cc.egui_ctx);

        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("wgpu レンダラーが必要です(NativeOptions.renderer を確認)");

        let dir = shader_dir();
        let mut canvas = GpuCanvas::new(
            &render_state.device,
            &render_state.queue,
            render_state.target_format,
            dir.clone(),
        );
        let shader_error = canvas.rebuild_pipelines(&render_state.device).err();
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(canvas);

        let params = SimParams::default();
        let [r, g, b, a] = params.brush_color.map(|c| (c * 255.0).round() as u8);
        Self {
            render_state,
            params,
            brush_color: egui::Color32::from_rgba_unmultiplied(r, g, b, a),
            stroke: StrokeState::default(),
            watcher: ShaderWatcher::new(&dir),
            shader_error,
        }
    }

    fn rebuild_shaders(&mut self) {
        let mut renderer = self.render_state.renderer.write();
        if let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() {
            self.shader_error = canvas
                .rebuild_pipelines(&self.render_state.device)
                .err();
        }
    }

    fn clear_canvas(&self) {
        let renderer = self.render_state.renderer.read();
        if let Some(canvas) = renderer.callback_resources.get::<GpuCanvas>() {
            canvas.clear(&self.render_state.queue);
        }
    }

    fn tool_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("ブラシ");
        ui.add(
            egui::Slider::new(&mut self.params.brush_radius, 1.0..=64.0)
                .text("半径")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.brush_flow, 0.0..=1.0).text("強さ"));
        ui.horizontal(|ui| {
            ui.label("色");
            ui.color_edit_button_srgba(&mut self.brush_color);
        });
        self.params.brush_color = self.brush_color.to_normalized_gamma_f32();

        ui.separator();
        if ui.button("キャンバスをリセット").clicked() {
            self.clear_canvas();
        }

        ui.separator();
        ui.heading("シェーダー (H1)");
        ui.label(format!("{} を監視中", shader_dir().display()));
        match &self.shader_error {
            None => {
                ui.colored_label(egui::Color32::from_rgb(64, 160, 64), "コンパイル OK");
            }
            Some(_) => {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "コンパイルエラー");
            }
        }
    }

    fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        // 正方形キャンバスを利用可能領域の中央に置く
        let available = ui.available_size();
        let side = available.min_elem().max(64.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(side, side),
            egui::Sense::drag(),
        );

        if response.drag_started() {
            self.stroke.begin();
        }

        let mut splats: Vec<Splat> = Vec::new();
        if response.drag_started() || response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let scale = CANVAS_SIZE as f32 / rect.width();
                let px = (pos - rect.min) * scale;
                let spacing = (self.params.brush_radius * 0.25).max(1.0);
                self.stroke
                    .add_motion([px.x, px.y], 1.0, spacing, &mut splats);
            }
        }
        if response.drag_stopped() {
            self.stroke.end();
        }

        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            CanvasCallback {
                params: self.params,
                splats,
            },
        ));
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            (1.0, ui.visuals().weak_text_color()),
            egui::StrokeKind::Outside,
        );
    }

    fn error_overlay(&self, ui: &mut egui::Ui) {
        let Some(error) = &self.shader_error else {
            return;
        };
        egui::Panel::bottom("shader_error")
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(60, 16, 16))
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 140, 140),
                    "WGSL コンパイルエラー(直前の正常なシェーダーで継続中。保存し直すと再試行):",
                );
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(error).monospace().color(egui::Color32::WHITE),
                        )
                        .wrap(),
                    );
                });
            });
    }
}

impl eframe::App for PaintApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // H1: .wgsl が保存されたら再ビルド(失敗しても落とさない)
        if self.watcher.take_dirty() {
            self.rebuild_shaders();
        }

        egui::Panel::left("tools")
            .default_size(260.0)
            .show(ui, |ui| self.tool_panel(ui));

        self.error_overlay(ui);

        egui::CentralPanel::default().show(ui, |ui| self.canvas_ui(ui));

        // M1 で常時シミュレーションが走るため、いまから連続再描画にしておく
        ui.ctx().request_repaint();
    }
}
