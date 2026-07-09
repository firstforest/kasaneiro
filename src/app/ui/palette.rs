//! パレット編集パネル(M5a/b)。app/mod.rs から分割(R4)。
//!
//! 固定 const だった 4 顔料を、色・ρ/ω/γ をその場で編集できるランタイムパレット
//! ([`pigment::Palette`])にする。編集したら [`PaintApp::apply_palette`] で GPU の
//! latents / physics バッファへ反映する(パイプライン再構築不要)。
//! ρ/ω/γ は湿シミュ専用なので即座に効き、色(latent)は現行(live)パレット枠だけ更新される。
//! 乾燥済みレイヤーは「乾かす」時に色を記録済みなので、後から顔料を編集しても変色しない(M5c)。

use crate::app::PaintApp;
use crate::pigment_store;
use eframe::egui;

impl PaintApp {
    /// パレット(M5): 選択中スロットの色・密度 ρ・ステイニング ω・粒状感 γ を編集する。
    /// 編集対象はブラシの顔料セレクタ(brush_panel の色スウォッチ)と連動する(M5g)
    pub(in crate::app) fn palette_panel(&mut self, ui: &mut egui::Ui) {
        // スポイトは色選びの動線なのでツールバー直後(brush_panel)へ移した。
        // 色/ρ/ω/γ の編集・ライブラリは折りたたみで通常時の情報量を下げる(F4)。
        // M5g: 中身を「選択中スロットだけ」に絞り(旧: 4スロット×4行の壁)、
        // パレット保存/読込・既定に戻すはパレットモーダル(file_menu)へ移設した
        ui.separator();
        egui::CollapsingHeader::new("色をつくる(選択中の色を編集)")
            .default_open(false)
            .show(ui, |ui| self.palette_details(ui));
    }

    /// スポイト待機トグル(色選びの動線)。M5e。brush_panel のツールバー直後から呼ばれる
    pub(in crate::app) fn eyedropper_control(&mut self, ui: &mut egui::Ui) {
        let slot = self.params.brush_channel.min(3) as usize + 1;
        let armed = self.palette_ui.eyedropper;
        if ui
            .selectable_label(armed, "💧 スポイト(画面の色を拾う)")
            .on_hover_text(format!(
                "押してからキャンバスをクリックすると、その点の色を選択中スロット #{slot} に取り込みます。\nI キーを押している間だけ一時的にスポイトにもできます(ペンタブレットのキーに I を割り当て)"
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

    /// 選択中スロットの色・性質(ρ/ω/γ)編集と、色ライブラリ/パレットへの動線。
    /// 折りたたみの中身(F4)。ラベルは平易な日本語を主に、数式記号 ρ/ω/γ はホバーへ温存(F2)。
    /// M5g: 編集対象を選択中スロット1つに絞る=ツールバーの色スウォッチが編集対象の切替を兼ねる
    fn palette_details(&mut self, ui: &mut egui::Ui) {
        let ch = self.params.brush_channel.min(3) as usize;
        ui.label(
            egui::RichText::new("編集する色は上の色ボタン(スウォッチ)で選びます")
                .weak()
                .small(),
        );

        let mut changed = false;
        let p = &mut self.palette.pigments[ch];
        ui.horizontal(|ui| {
            changed |= ui.color_edit_button_srgb(&mut p.rgb).changed();
            ui.label(format!("#{}", ch + 1));
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

        if changed {
            self.apply_palette();
        }

        // 色ライブラリ(M5f)・パレット(M5g)への動線。
        // 色選びはツール文脈なので左パネル、一覧付きのファイル操作はモーダル(file_menu)に置く
        ui.horizontal(|ui| {
            let current = self.palette.pigments[ch].clone();
            let name = current.name.trim().to_owned();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("この色をとっておく"))
                .on_hover_text(
                    "この色(名前・色・性質)を色ライブラリへ1クリック保存します(同名は上書き)",
                )
                .clicked()
            {
                self.status_msg = Some(match pigment_store::save(&name, &current) {
                    Ok(path) => format!("保存: {}", path.display()),
                    Err(e) => e,
                });
            }
            if ui
                .button("色ライブラリ…")
                .on_hover_text("とっておいた色の一覧から選択中スロットへ読み込みます")
                .clicked()
            {
                self.open_pigment_modal();
            }
            if ui
                .button("パレット…")
                .on_hover_text("4色一式の保存/読込(パレットモーダル)")
                .clicked()
            {
                self.open_palette_modal();
            }
        });
        ui.label(
            egui::RichText::new(
                "※色は「乾かす(固定)」時にその層へ記録され、以降編集しても乾いた層の色は変わりません",
            )
            .weak(),
        );
    }
}
