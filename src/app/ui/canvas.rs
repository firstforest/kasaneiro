//! 中央のキャンバス描画(ポインタ入力の取り込み・記録再生の合流・シミュステップ数の決定)と
//! シェーダーエラーのオーバーレイ。app/mod.rs から分割(R4)。

use crate::app::PaintApp;
use crate::gpu::CanvasCallback;
use crate::input::PointerSource;
use paint_core::sim::Splat;
use eframe::egui;
use eframe::egui_wgpu;

impl PaintApp {
    pub(in crate::app) fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        // 正方形キャンバスを利用可能領域の中央に置く
        let available = ui.available_size();
        let side = available.min_elem().max(64.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(side, side),
            egui::Sense::drag(),
        );

        // M6: パン/ズーム。ホイールでカーソル中心に拡大、中ボタンドラッグでパン。
        // ポインタ状態はグローバル(ウィジェットの Sense に依らない)なので input から直接読む
        let (scroll_y, middle_down, ptr_delta, hover) = ui.input(|i| {
            (
                i.smooth_scroll_delta.y,
                i.pointer.middle_down(),
                i.pointer.delta(),
                i.pointer.hover_pos(),
            )
        });
        let over_canvas = hover.is_some_and(|p| rect.contains(p));
        if over_canvas
            && scroll_y != 0.0
            && let Some(cursor) = hover
        {
            // ホイール量を対数スケールで拡大率に。上スクロール=拡大 / 下=縮小
            self.zoom_at(cursor, rect, (scroll_y * 0.0015).exp());
        }
        // パンは拡大中(zoom>1)のみ意味を持つ。中ボタンをキャンバス上で押している間に移動量を反映
        let panning = middle_down && over_canvas && self.view_zoom > 1.0;
        if panning {
            let span = 1.0 / self.view_zoom;
            self.view_offset -= (ptr_delta / rect.width().max(1.0)) * span;
            self.clamp_view();
        }

        // M1.5: ペン(egui Touch、筆圧付き)を優先し、接地中はマウスを無視する
        // (egui-winit は Touch からポインタもエミュレートするため、両方を処理すると
        // 二重ストロークになる)
        let pen_events = self.pen.poll(&response);
        let events = if self.pen.is_active() || !pen_events.is_empty() {
            pen_events
        } else {
            self.mouse.poll(&response)
        };

        let mut splats: Vec<Splat> = Vec::new();
        // M5e: スポイト待機中はクリックで色を拾うだけで、描画・記録はしない
        if self.palette_ui.eyedropper {
            let pressed = ui.input(|i| i.pointer.primary_pressed());
            if pressed
                && let Some(pos) = response.hover_pos()
                && rect.contains(pos)
            {
                self.pick_color(pos, rect);
                self.palette_ui.eyedropper = false;
            }
        } else if !panning {
            // M6: パン中(中ボタンドラッグ)は描画イベントを流さない(ビュー操作専念)
            self.apply_pointer_events(&events, rect, &mut splats);
        }

        // H5: 記録はフレーム基準(ストローク間の待ちも再現される)
        if let Some(recorder) = &mut self.replay_ui.recorder {
            recorder.tick();
        }

        // H5: 再生中は記録済みポインタ入力を同じテンポで流し込む(手描きと合流可)
        if let Some(player) = &mut self.replay_ui.player
            && !player.advance(&mut self.params, &mut splats)
        {
            self.replay_ui.player = None;
        }

        // H6: 一時停止中は 0 ステップ(1ステップボタンが押されていれば 1)
        let sim_steps = if self.paused {
            if std::mem::take(&mut self.step_once) { 1 } else { 0 }
        } else {
            self.steps_per_frame
        };

        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            CanvasCallback {
                params: self.params,
                splats,
                sim_steps,
                line_target: self.line_target(),
                view: self.view_uniform(),
            },
        ));
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            (1.0, ui.visuals().weak_text_color()),
            egui::StrokeKind::Outside,
        );
    }

    pub(in crate::app) fn error_overlay(&self, ui: &mut egui::Ui) {
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
