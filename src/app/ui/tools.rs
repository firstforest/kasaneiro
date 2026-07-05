//! 乾燥ボタン(常時表示)と水ブラシパネル。app/mod.rs から分割(R4)。

use crate::app::PaintApp;
use paint_core::tool::{RasterTool, Tool, ToolInfo, WetTool};
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
        // 顔料セレクタ(M1c/M5): ブラシが注入する顔料スロット。色・名前・個性はランタイム
        // パレット(self.palette)から。編集はパレットパネル(palette_panel)で行う。
        // スロット情報を先にスナップショットしておく(ループ内で self.params を触るための借用回避)
        let swatches: Vec<(egui::Color32, String)> = self
            .palette
            .pigments
            .iter()
            .map(|p| {
                (
                    egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]),
                    format!(
                        "{}\n密度 ρ={:.2} / ステイニング ω={:.2} / 粒状感 γ={:.2}",
                        p.name, p.density, p.staining, p.granulation
                    ),
                )
            })
            .collect();
        ui.horizontal(|ui| {
            for (i, (color, hover)) in swatches.iter().enumerate() {
                let selected = self.params.brush_channel == i as u32;
                let mut button = egui::Button::new("").fill(*color).min_size(egui::vec2(28.0, 28.0));
                if selected {
                    button = button.stroke((2.0, ui.visuals().strong_text_color()));
                }
                if ui.add(button).on_hover_text(hover.clone()).clicked() {
                    self.params.brush_channel = i as u32;
                }
            }
        });
        let pg = &self.palette.pigments[self.params.brush_channel.min(3) as usize];
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

    /// 線画(M4.5a): 鉛筆/ペンの選択・消しゴム・線のスライダー。流体を通らないラスタツール。
    /// 選択したら kind を params.line_mode / eraser を params.line_eraser へ同期し
    /// linesplat.wgsl の分岐に渡す(描画先テクスチャの選択は CanvasCallback 経由)
    pub(in crate::app) fn linework_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("線画 (M4.5)");
        // 現在の消しゴム状態を引き継いでツールを切り替える(ラスタでなければ描画から始める)
        let cur_eraser = matches!(self.tool, Tool::Raster { eraser: true, .. });
        ui.horizontal(|ui| {
            for kind in RasterTool::ALL {
                let selected = matches!(self.tool, Tool::Raster { kind: k, .. } if k == kind);
                if ui
                    .selectable_label(selected, kind.label())
                    .on_hover_text(kind.hint())
                    .clicked()
                {
                    self.tool = Tool::Raster { kind, eraser: cur_eraser };
                }
            }
            // 消しゴムはラスタツール選択中のみ有効
            let is_raster = matches!(self.tool, Tool::Raster { .. });
            let mut eraser = cur_eraser;
            if ui
                .add_enabled(is_raster, egui::Checkbox::new(&mut eraser, "消しゴム"))
                .changed()
                && let Tool::Raster { kind, .. } = self.tool
            {
                self.tool = Tool::Raster { kind, eraser };
            }
        });
        // ツール状態を GPU パラメータへ同期(ラスタ選択中のみ)
        if let Tool::Raster { kind, eraser } = self.tool {
            self.params.line_mode = match kind {
                RasterTool::Pencil => 0,
                RasterTool::Pen => 1,
                RasterTool::Highlight => 2,
            };
            self.params.line_eraser = eraser as u32;
        }
        // 鉛筆/ペンは太さ・濃さを独立に持つ(水ブラシの brush_radius とは別)
        ui.label(egui::RichText::new("鉛筆").strong());
        ui.add(
            egui::Slider::new(&mut self.params.pencil_radius, 1.0..=64.0)
                .text("太さ")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.pencil_strength, 0.0..=1.0).text("濃さ"));
        ui.add(egui::Slider::new(&mut self.params.pencil_gran, 0.0..=1.0).text("粒状感"));
        ui.label(egui::RichText::new("ペン").strong());
        ui.add(
            egui::Slider::new(&mut self.params.pen_radius, 1.0..=64.0)
                .text("太さ")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.pen_strength, 0.0..=1.0).text("濃さ"));
        ui.add(
            egui::Slider::new(&mut self.params.line_block, 0.0..=1.0)
                .text("ペン線の透水率(水の境界)"),
        )
        .on_hover_text(
            "清書ペンの線を水の境界にする強さ(M4.5b)。上げるほど、ペンで囲った領域を塗っても水がはみ出さない。0=境界なし。線を跨いでストロークすれば明示的に越えられる",
        );
        ui.label(egui::RichText::new("ハイライト").strong());
        ui.add(
            egui::Slider::new(&mut self.params.highlight_radius, 1.0..=64.0)
                .text("太さ")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.highlight_strength, 0.0..=1.0).text("不透明度"));
        ui.label("※筆圧の効きは「筆圧」パネルの値を共用します");
        // 線画の多段 Undo/Redo(M4.5d)
        self.line_history_controls(ui);
    }

    /// 線画の多段 Undo/Redo(M4.5d): ボタン+履歴本数の表示。キーは Ctrl+Z / Ctrl+Shift+Z。
    /// 湿レイヤー(水彩)は対象外(M6 の 1 段 undo で扱う)
    pub(in crate::app) fn line_history_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let can_undo = !self.line_history.done.is_empty();
            let can_redo = !self.line_history.redo.is_empty();
            if ui
                .add_enabled(can_undo, egui::Button::new("↶ 元に戻す"))
                .on_hover_text("線画を1本戻す (Ctrl+Z)。水彩は対象外")
                .clicked()
            {
                self.line_undo();
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("↷ やり直し"))
                .on_hover_text("取り消した線画を1本復元 (Ctrl+Shift+Z)")
                .clicked()
            {
                self.line_redo();
            }
        });
        ui.label(format!(
            "線画履歴: {} 本(やり直し {} 本)",
            self.line_history.done.len(),
            self.line_history.redo.len()
        ));
    }
}
