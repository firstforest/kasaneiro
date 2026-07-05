//! パレット編集パネル(M5a/b)。app/mod.rs から分割(R4)。
//!
//! 固定 const だった 4 顔料を、色・ρ/ω/γ をその場で編集できるランタイムパレット
//! ([`pigment::Palette`])にする。編集したら [`PaintApp::apply_palette`] で GPU の
//! latents / physics バッファへ反映する(パイプライン再構築不要)。
//! ρ/ω/γ は湿シミュ専用なので即座に効き、色(latent)は現行(live)パレット枠だけ更新される。
//! 乾燥済みレイヤーは「乾かす」時に色を記録済みなので、後から顔料を編集しても変色しない(M5c)。

use crate::app::PaintApp;
use eframe::egui;

impl PaintApp {
    /// パレット(M5): 4スロットの色・密度 ρ・ステイニング ω・粒状感 γ を編集する。
    /// ブラシの顔料セレクタ(brush_panel)と同じ4スロットを指す
    pub(in crate::app) fn palette_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("パレット (M5)");
        ui.label("色と顔料の性質(沈み方・剥がれ方・粒状感)を編集できます");

        let mut changed = false;
        for (i, p) in self.palette.pigments.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                changed |= ui.color_edit_button_srgb(&mut p.rgb).changed();
                ui.label(format!("#{}", i + 1));
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut p.name).desired_width(140.0))
                    .changed();
            });
            // per-顔料の密度 ρ は「沈着の速さ」。表示の被覆率カーブ(SimParams の
            // pigment_density)とは別概念なので用語を分ける(M5a)
            changed |= ui
                .add(egui::Slider::new(&mut p.density, 0.1..=2.0).text("密度 ρ(沈む速さ)"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut p.staining, 0.0..=1.0).text("ステイニング ω(残りやすさ)"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut p.granulation, 0.0..=1.0).text("粒状感 γ(紙目に溜まる)"))
                .changed();
            ui.separator();
        }

        if ui
            .button("パレットを既定に戻す")
            .on_hover_text("4スロットを起動時の顔料に戻す")
            .clicked()
        {
            self.palette = pigment::Palette::default_palette();
            changed = true;
        }
        ui.label(
            egui::RichText::new(
                "※色は「乾かす」時にレイヤーへ記録され、以降編集しても乾燥済みレイヤーの色は変わりません(M5c)",
            )
            .weak(),
        );

        if changed {
            self.apply_palette();
        }
    }
}
