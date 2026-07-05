//! 乾燥ボタン(常時表示)と水ブラシパネル。app/mod.rs から分割(R4)。

use crate::app::PaintApp;
use paint_core::tool::{Tool, ToolInfo, WetTool};
use pigment::PIGMENTS;
use eframe::egui;

impl PaintApp {
    /// M2: 乾燥操作は「にじみを止めたい瞬間」に間に合う必要がある(Fresco の UX 教訓)ため、
    /// スクロール領域の外=左パネル最上部に常時表示する
    pub(in crate::app) fn dry_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .button(egui::RichText::new("乾かす").strong())
                .on_hover_text("定着パスを走らせて乾燥レイヤーへ焼き込み、湿レイヤーを空にする(M2)")
                .clicked()
            {
                self.run_canvas_action(|c, d, q| c.bake_dry(d, q));
            }
            if ui
                .button("水だけ除去")
                .on_hover_text("Fast Dry: 水と流れを止め、浮遊顔料をその場で沈着(焼き込みはしない)")
                .clicked()
            {
                self.run_canvas_action(|c, d, q| c.fast_dry(d, q));
            }
            if ui
                .button("全面を湿らす")
                .on_hover_text("Wet the Layer: キャンバス全面を濡らす(水量は「再湿潤の水量」スライダー)")
                .clicked()
            {
                self.run_canvas_action(|c, d, q| c.rewet(d, q));
            }
        });
        ui.add_space(4.0);
    }

    /// 水ブラシ(M1〜M4): ツール選択・顔料スロット・ブラシスライダー
    pub(in crate::app) fn brush_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("水ブラシ");
        // ツール選択(R2): WetTool を回してボタン化する。ラベル・文言・GPU 値は
        // enum の impl に一元化されている(TOOLS 定数表は廃止)。選択したら
        // gpu_id を params.tool へ同期して splat.wgsl の分岐に渡す
        ui.horizontal(|ui| {
            for wt in WetTool::ALL {
                let selected = self.tool == Tool::Wet(wt);
                if ui
                    .selectable_label(selected, wt.label())
                    .on_hover_text(wt.hint())
                    .clicked()
                {
                    self.tool = Tool::Wet(wt);
                }
            }
        });
        if let Some(wt) = self.tool.wet() {
            self.params.tool = wt.gpu_id();
        }
        // 顔料セレクタ(M1c): ブラシが注入する顔料スロットを選ぶ。ホバーで顔料個性(M3)を表示
        ui.horizontal(|ui| {
            for (i, pigment) in PIGMENTS.iter().enumerate() {
                let selected = self.params.brush_channel == i as u32;
                let color =
                    egui::Color32::from_rgb(pigment.rgb[0], pigment.rgb[1], pigment.rgb[2]);
                let mut button = egui::Button::new("").fill(color).min_size(egui::vec2(28.0, 28.0));
                if selected {
                    button = button.stroke((2.0, ui.visuals().strong_text_color()));
                }
                let hover = format!(
                    "{}\n密度 ρ={:.2} / ステイニング ω={:.2} / 粒状感 γ={:.2}",
                    pigment.name, pigment.density, pigment.staining, pigment.granulation
                );
                if ui.add(button).on_hover_text(hover).clicked() {
                    self.params.brush_channel = i as u32;
                }
            }
        });
        let pg = &PIGMENTS[self.params.brush_channel.min(3) as usize];
        ui.label(format!(
            "{}(ω={:.2} γ={:.2})",
            pg.name, pg.staining, pg.granulation
        ));
        ui.add(
            egui::Slider::new(&mut self.params.brush_radius, 1.0..=64.0)
                .text("半径")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.brush_water, 0.0..=2.0).text("水量"));
        ui.add(egui::Slider::new(&mut self.params.brush_velocity, 0.0..=2.0).text("初速"));
        ui.add(egui::Slider::new(&mut self.params.brush_pigment, 0.0..=1.0).text("顔料量(0=水筆)"));
        ui.add(
            egui::Slider::new(&mut self.params.lift_strength, 0.0..=1.0)
                .text("リフト強度(削りツール)"),
        );
        ui.add(
            egui::Slider::new(&mut self.params.water_lift, 0.0..=1.0)
                .text("水筆の均し強度(均一さ)"),
        );
        ui.add(
            egui::Slider::new(&mut self.params.smear_rate, 0.0..=1.0)
                .text("ならし強度(濃い山を伸ばす)"),
        );
    }
}
