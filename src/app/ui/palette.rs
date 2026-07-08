//! パレット編集パネル(M5a/b)。app/mod.rs から分割(R4)。
//!
//! 固定 const だった 4 顔料を、色・ρ/ω/γ をその場で編集できるランタイムパレット
//! ([`pigment::Palette`])にする。編集したら [`PaintApp::apply_palette`] で GPU の
//! latents / physics バッファへ反映する(パイプライン再構築不要)。
//! ρ/ω/γ は湿シミュ専用なので即座に効き、色(latent)は現行(live)パレット枠だけ更新される。
//! 乾燥済みレイヤーは「乾かす」時に色を記録済みなので、後から顔料を編集しても変色しない(M5c)。

use crate::app::PaintApp;
use crate::palette_store;
use eframe::egui;

impl PaintApp {
    /// パレット(M5): 4スロットの色・密度 ρ・ステイニング ω・粒状感 γ を編集する。
    /// ブラシの顔料セレクタ(brush_panel)と同じ4スロットを指す
    pub(in crate::app) fn palette_panel(&mut self, ui: &mut egui::Ui) {
        // スポイトは色選びの動線なので折りたたみの外(常時)に置く。
        // 色/ρ/ω/γ の編集・ライブラリは「顔料の詳細設定」に畳んで通常時の情報量を下げる(F4)。
        ui.separator();
        self.eyedropper_control(ui);
        egui::CollapsingHeader::new("顔料の詳細設定(色・沈み方・粒状感)")
            .default_open(false)
            .show(ui, |ui| self.palette_details(ui));
    }

    /// スポイト待機トグル(色選びの動線なので常時表示)。M5e
    fn eyedropper_control(&mut self, ui: &mut egui::Ui) {
        let slot = self.params.brush_channel.min(3) as usize + 1;
        let armed = self.palette_ui.eyedropper;
        if ui
            .selectable_label(armed, "💧 スポイト(画面の色を拾う)")
            .on_hover_text(format!(
                "押してからキャンバスをクリックすると、その点の色を選択中スロット #{slot} に取り込みます"
            ))
            .clicked()
        {
            self.palette_ui.eyedropper = !armed;
        }
        if self.palette_ui.eyedropper {
            ui.colored_label(
                egui::Color32::from_rgb(64, 130, 200),
                "スポイト待機中… キャンバスをクリックしてください",
            );
        }
    }

    /// 顔料の色・性質(ρ/ω/γ)編集とパレット・ライブラリ。折りたたみの中身(F4)。
    /// ラベルは平易な日本語を主に、数式記号 ρ/ω/γ はホバーへ温存(F2)
    fn palette_details(&mut self, ui: &mut egui::Ui) {
        ui.label("色と顔料の性質(沈み方・染みつき・粒状感)を編集できます");

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
                .add(egui::Slider::new(&mut p.density, 0.1..=2.0).text("沈みやすさ"))
                .on_hover_text("密度 ρ: 大きいほど早く紙に沈着する")
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut p.staining, 0.0..=1.0).text("染みつき(落ちにくさ)"))
                .on_hover_text("ステイニング ω: 大きいほど削りで落ちず紙に残る")
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut p.granulation, 0.0..=1.0).text("粒状感(紙目のザラつき)"))
                .on_hover_text("粒状感 γ: 大きいほど紙の凹部に溜まりザラつく")
                .changed();
            ui.separator();
        }

        if ui
            .button("既定に戻す")
            .on_hover_text("4スロットを起動時の顔料に戻す")
            .clicked()
        {
            self.palette = pigment::Palette::default_palette();
            changed = true;
        }
        ui.label(
            egui::RichText::new(
                "※色は「乾かす(固定)」時にその層へ記録され、以降編集しても乾いた層の色は変わりません",
            )
            .weak(),
        );

        if changed {
            self.apply_palette();
        }

        // パレット・ライブラリ: 現行パレットに名前を付けて保存 / 一覧から読込。
        // 保存/一覧の共通 UI は NamedStore(プリセット H3 と同じ流儀)
        ui.separator();
        ui.label(egui::RichText::new("パレットの保存").strong());
        let palette = self.palette.clone();
        if let Some(status) = self.palette_ui.store.save_controls(
            ui,
            "パレット名",
            |name| palette_store::save(name, &palette),
            palette_store::list,
        ) {
            self.status_msg = Some(status);
        }
        if let Some(name) = self.palette_ui.store.list_rows(ui, "読込") {
            match palette_store::load(&name) {
                Ok(loaded) => {
                    self.palette = loaded;
                    self.apply_palette();
                }
                Err(e) => self.status_msg = Some(e),
            }
        }
    }
}
