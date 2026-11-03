//! 混色皿: 右パネルの小さな湿レイヤー([`crate::gpu::MixCanvas`])。本体と同じ筆・同じツールで塗って
//! 色を混ぜておき、スポイトか乾きかけの筆でなでて拾う。筆の中身のバッファは本体と共有しているので、
//! 皿で塗れば筆が減り、皿の色水を吸えば筆にその色が入る。
//!
//! 本体のキャンバスと違い、ストロークは記録(H5・解析用キャプチャ)にも Undo にも載せない。
//! 皿は拡大・回転せず、1 テクセル = 皿の幅 / [`MIX_CANVAS_SIZE`] の画面 px で写す。

use crate::app::{PaintApp, Surface, mixpad_texel};
use crate::gpu::{CanvasCallback, MIX_CANVAS_SIZE, MixCallback, ViewUniform};
use crate::input::{PointerEvent, PointerPhase, PointerSource};
use eframe::egui;
use eframe::egui_wgpu;
use paint_core::load::BrushFrame;
use paint_core::sim::Splat;
use paint_core::tool::{Tool, WetTool};

impl PaintApp {
    /// 混色皿の見出し・洗うボタンと皿本体
    /// 折りたたむと皿は描かれず、皿の時間(乾き・にじみ)も止まる
    pub(in crate::app) fn mixpad_panel(&mut self, ui: &mut egui::Ui) {
        let shown = egui::CollapsingHeader::new("混色皿")
            .default_open(true)
            .show(ui, |ui| self.mixpad_body(ui));
        shown.header_response.on_hover_text(
            "キャンバスと同じ筆で塗って色を混ぜておく場所。\
             スポイトで拾うか、乾きかけの筆で濡れた色をなでると筆に取れる。\
             パレットを替えると皿の色も替わる。閉じている間は皿の乾きも止まる",
        );
        if shown.body_returned.is_none() {
            self.end_mixpad_stroke();
        }
    }

    fn mixpad_body(&mut self, ui: &mut egui::Ui) {
        if ui
            .small_button("皿を洗う")
            .on_hover_text("皿の絵の具と水を全部落として白に戻す")
            .clicked()
        {
            self.end_mixpad_stroke();
            self.clear_mixpad();
        }
        let side = ui.available_width().clamp(64.0, MIX_CANVAS_SIZE as f32);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::drag());
        // 支援技術向けの名前(e2e テストもこの名前で皿の位置を引く)
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "混色皿の紙"));

        let pen_events = self.mixpad.pen.poll(&response);
        let events = if self.mixpad.pen.is_active() || !pen_events.is_empty() {
            pen_events
        } else {
            self.mixpad.mouse.poll(&response)
        };
        let hover = response.hover_pos().filter(|p| rect.contains(*p));

        let mut splats: Vec<Splat> = Vec::new();
        if let Some(target) = self.palette_ui.eyedropper {
            // スポイト待機中はクリックで拾うだけ
            self.end_mixpad_stroke();
            if ui.input(|i| i.pointer.primary_pressed())
                && let Some(pos) = hover
            {
                self.palette_ui.eyedropper = None;
                self.pick(target, Surface::MixPad, pos, rect);
            }
        } else if self.tool.wet().is_some() && !self.painting {
            self.mixpad_pointer_events(&events, rect, &mut splats);
            // 置いたまま動かないフレームも筆と紙の受け渡しを続ける(本体の hold_still と同じ)
            if self.mixpad.painting
                && self.tool == Tool::Wet(WetTool::Paint)
                && splats.is_empty()
                && let Some((pos, pressure)) = self.mixpad.feed_pos
            {
                splats.push(Splat::feed_at(pos, pressure));
            }
        } else {
            self.end_mixpad_stroke();
        }

        let mut brush = BrushFrame::default();
        if self.tool == Tool::Wet(WetTool::Paint) && !splats.is_empty() {
            brush.drag(&mut splats, &self.params);
            brush.dt = self.brush_clock.take();
        }
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            MixCallback(CanvasCallback {
                params: self.params,
                splats,
                sim_steps: self.frame_sim_steps,
                line_target: None,
                view: ViewUniform::default(),
                brush,
            }),
        ));
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            (1.0, ui.visuals().weak_text_color()),
            egui::StrokeKind::Outside,
        );

        // 筆の太さのリング(本体のキャンバスと同じ見た目)
        if let Some(pos) = hover
            && self.palette_ui.eyedropper.is_none()
            && self.tool.wet().is_some()
        {
            let scale = rect.width() / MIX_CANVAS_SIZE as f32;
            let r = (self.params.brush_radius * scale).max(1.5);
            let painter = ui.painter().with_clip_rect(rect);
            painter.circle_stroke(pos, r + 0.5, (1.5, egui::Color32::from_black_alpha(140)));
            painter.circle_stroke(pos, r, (1.0, egui::Color32::from_white_alpha(210)));
        }
    }

    /// 混色皿のポインタイベントをストロークと splat 列へ反映する
    fn mixpad_pointer_events(
        &mut self,
        events: &[PointerEvent],
        rect: egui::Rect,
        splats: &mut Vec<Splat>,
    ) {
        for ev in events {
            match ev.phase {
                PointerPhase::Down => {
                    if !rect.contains(ev.pos) {
                        continue;
                    }
                    self.mixpad.painting = true;
                    self.mixpad.stroke.begin();
                    if self.auto_refill {
                        self.set_brush(self.brush_refill);
                    }
                    log::debug!("混色皿: ストローク開始 {:?}", self.tool);
                }
                PointerPhase::Move => {}
                PointerPhase::Up => {
                    self.end_mixpad_stroke();
                    continue;
                }
            }
            if !self.mixpad.painting {
                continue;
            }
            let px = mixpad_texel(ev.pos, rect);
            let base = self.params.brush_radius;
            let params = &self.params;
            self.mixpad.stroke.add_motion(
                [px.x, px.y],
                ev.pressure,
                |p| params.splat_spacing(base, p),
                splats,
            );
            self.mixpad.feed_pos = Some(([px.x, px.y], ev.pressure));
        }
    }

    fn end_mixpad_stroke(&mut self) {
        if self.mixpad.painting {
            log::debug!("混色皿: ストローク終了");
        }
        self.mixpad.painting = false;
        self.mixpad.stroke.end();
        self.mixpad.feed_pos = None;
    }
}
