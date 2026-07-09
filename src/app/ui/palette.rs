//! パレット編集パネル(M5a/b)。app/mod.rs から分割(R4)。
//!
//! 固定 const だった 4 顔料を、色・ρ/ω/γ をその場で編集できるランタイムパレット
//! ([`pigment::Palette`])にする。編集したら [`PaintApp::apply_palette`] で GPU の
//! latents / physics バッファへ反映する(パイプライン再構築不要)。
//! ρ/ω/γ は湿シミュ専用なので即座に効き、色(latent)は現行(live)パレット枠だけ更新される。
//! 乾燥済みレイヤーは「乾かす」時に色を記録済みなので、後から顔料を編集しても変色しない(M5c)。
//!
//! 色作り・パレット編集はメイン機能なので**常時表示**(モーダル・折りたたみに隠さない):
//! 「色をつくる」(選択中スロットの編集)+「色ライブラリ」(1色のスウォッチグリッド)+
//! 「パレット」(4色一式の保存/読込)を左パネルに並べる。旧 FileModal::Palette / Pigment は廃止。

use crate::app::PaintApp;
use crate::palette_store;
use crate::pigment_store;
use eframe::egui;

/// 4色見本チップ(保存済みパレット一覧の行用)。クリック不可の小さな色角丸
fn palette_chips(ui: &mut egui::Ui, palette: &pigment::Palette) {
    for p in &palette.pigments {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
        ui.painter().rect_filled(
            rect,
            3.0,
            egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]),
        );
    }
}

impl PaintApp {
    /// パレット(M5): 色をつくる・色ライブラリ・パレットの3セクションを常時表示する。
    /// 編集対象はブラシの顔料セレクタ(brush_panel の色スウォッチ)と連動する(M5g)
    pub(in crate::app) fn palette_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        self.color_edit_section(ui);
        ui.separator();
        self.pigment_library_section(ui);
        ui.separator();
        self.palette_file_section(ui);
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

    /// 色をつくる: 選択中スロットの色・性質(ρ/ω/γ)編集と1クリック保存。
    /// ラベルは平易な日本語を主に、数式記号 ρ/ω/γ はホバーへ温存(F2)。
    /// M5g: 編集対象を選択中スロット1つに絞る=ツールバーの色スウォッチが編集対象の切替を兼ねる
    fn color_edit_section(&mut self, ui: &mut egui::Ui) {
        let ch = self.params.brush_channel.min(3) as usize;
        ui.strong("色をつくる");
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

        // 1クリック保存(M5f。保存名=顔料名の1本化)。保存すると下のライブラリに即現れる
        let current = self.palette.pigments[ch].clone();
        let name = current.name.trim().to_owned();
        if ui
            .add_enabled(!name.is_empty(), egui::Button::new("この色をとっておく"))
            .on_hover_text(
                "この色(名前・色・性質)を下の色ライブラリへ1クリック保存します(同名は上書き)",
            )
            .clicked()
        {
            self.status_msg = Some(match pigment_store::save(&name, &current) {
                Ok(path) => {
                    self.palette_ui.pigment_cache = pigment_store::load_all();
                    format!("保存: {}", path.display())
                }
                Err(e) => e,
            });
        }
        ui.label(
            egui::RichText::new(
                "※色は「乾かす(固定)」時にその層へ記録され、以降編集しても乾いた層の色は変わりません",
            )
            .weak()
            .small(),
        );
    }

    /// 色ライブラリ(M5f): とっておいた1色のスウォッチグリッド。クリックで選択中スロットへ
    /// 読み込む(ツールバーの色スウォッチと同じ意味論=brush_channel)。旧 FileModal::Pigment の常設化
    fn pigment_library_section(&mut self, ui: &mut egui::Ui) {
        let ch = self.params.brush_channel.min(3) as usize;
        ui.horizontal(|ui| {
            ui.strong("色ライブラリ");
            if ui.button("↻").on_hover_text("一覧を再読込").clicked() {
                self.palette_ui.pigment_cache = pigment_store::load_all();
            }
        });
        if self.palette_ui.pigment_cache.is_empty() {
            ui.label(
                egui::RichText::new("とっておいた色はまだありません(「この色をとっておく」で追加)")
                    .weak()
                    .small(),
            );
            return;
        }
        ui.label(
            egui::RichText::new(format!("クリックで選択中のスロット #{} に入れます", ch + 1))
                .weak()
                .small(),
        );
        let mut do_load: Option<(String, pigment::Pigment)> = None;
        ui.horizontal_wrapped(|ui| {
            for (name, p) in &self.palette_ui.pigment_cache {
                let button = egui::Button::new("")
                    .fill(egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]))
                    .corner_radius(4.0)
                    .min_size(egui::vec2(24.0, 24.0));
                if ui
                    .add(button)
                    .on_hover_text(format!(
                        "{name}\n沈みやすさ {:.2} / 染みつき {:.2} / 粒状感 {:.2}\nクリックでスロット #{} に読み込みます",
                        p.density,
                        p.staining,
                        p.granulation,
                        ch + 1
                    ))
                    .clicked()
                {
                    do_load = Some((name.clone(), p.clone()));
                }
            }
        });
        if let Some((name, p)) = do_load {
            self.palette.pigments[ch] = p;
            self.apply_palette();
            self.status_msg = Some(format!("スロット #{} ← {name}", ch + 1));
        }
    }

    /// パレット(M5g): いまの4色一式の名前付き保存と、保存済み一覧(4色チップ)からの読込。
    /// 旧 FileModal::Palette の常設化。読込しても乾いた層の色は変わらない(M5c)
    fn palette_file_section(&mut self, ui: &mut egui::Ui) {
        ui.strong("パレット(4色一式)");

        // 保存: 名前欄+[保存]+↻。保存(成功)後は一覧チップも最新化する
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.palette_ui.store.name)
                    .hint_text("パレット名")
                    .desired_width(120.0),
            );
            let name = self.palette_ui.store.name.trim().to_owned();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("保存"))
                .on_hover_text("いまの4色一式に名前を付けて保存します(同名は上書き)")
                .clicked()
            {
                self.status_msg = Some(match palette_store::save(&name, &self.palette) {
                    Ok(path) => {
                        self.palette_ui.palette_cache = palette_store::load_all();
                        format!("保存: {}", path.display())
                    }
                    Err(e) => e,
                });
            }
            if ui.button("↻").on_hover_text("一覧を再読込").clicked() {
                self.palette_ui.palette_cache = palette_store::load_all();
            }
        });

        // 読込: 4色チップ付き一覧。今のパレットが未保存なら上書きされる点はホバーで注意
        if self.palette_ui.palette_cache.is_empty() {
            ui.label(egui::RichText::new("保存されたパレットはありません").weak().small());
        }
        let mut do_load: Option<(String, pigment::Palette)> = None;
        for (name, pal) in &self.palette_ui.palette_cache {
            ui.horizontal(|ui| {
                if ui
                    .button("読込")
                    .on_hover_text("4色一式を丸ごと入れ替えます(今の4色が惜しければ先に保存を)")
                    .clicked()
                {
                    do_load = Some((name.clone(), pal.clone()));
                }
                palette_chips(ui, pal);
                ui.label(name);
            });
        }
        if let Some((name, pal)) = do_load {
            self.palette = pal;
            self.apply_palette();
            self.status_msg = Some(format!("パレット読込: {name}"));
        }

        if ui
            .button("既定のパレットに戻す")
            .on_hover_text("4スロットを起動時の顔料に戻す")
            .clicked()
        {
            self.palette = pigment::Palette::default_palette();
            self.apply_palette();
            self.status_msg = Some("既定のパレットに戻しました".to_owned());
        }
    }
}
