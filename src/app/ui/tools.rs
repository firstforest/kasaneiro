//! 乾燥ボタン(常時表示)と、アクティブレイヤーごとのツールパネル。app/mod.rs から分割(R4)。
//!
//! レイヤーごとに使うツールが決まっているため、左パネルのツール群は右のレイヤーパネルで
//! 選択中のレイヤーに関係するものだけを出す([`PaintApp::active_tools_panel`] が出し分ける):
//! 水彩 → 水ブラシ+パレット / 鉛筆・ペン・ハイライト → 各線画ツール / 乾燥 → 編集不可の案内。

use crate::app::{ActiveLayer, PaintApp};
use crate::gpu::GpuCanvas;
use paint_core::tool::{RasterTool, Tool, ToolInfo, WetTool};
use eframe::egui;

impl PaintApp {
    /// 選択中レイヤーのツールパネル(左パネル先頭)。右のレイヤーパネルの選択で出し分ける。
    /// F12: 「今のツール」のブロックを Frame::group で囲み、下の設定セクションと視覚的に分離する
    pub(in crate::app) fn active_tools_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            match self.active_layer {
                ActiveLayer::Wet => {
                    self.brush_panel(ui);
                    self.palette_panel(ui);
                }
                ActiveLayer::Pencil => self.pencil_panel(ui),
                ActiveLayer::Pen => self.pen_panel(ui),
                ActiveLayer::Highlight => self.highlight_panel(ui),
                ActiveLayer::Dried(index) => self.dried_info_panel(ui, index),
            }
        });
    }

    /// M2: 乾燥操作は「にじみを止めたい瞬間」に間に合う必要がある(Fresco の UX 教訓)ため、
    /// スクロール領域の外=左パネル最上部に常時表示する
    pub(in crate::app) fn dry_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .button(egui::RichText::new("乾かす(固定)").strong())
                .on_hover_text("定着パス(bake)を走らせて乾燥レイヤーへ焼き込み、湿レイヤーを空にする。色は乾いた層として固定される(M2)")
                .clicked()
            {
                self.run_canvas_action(|c, d, q| c.bake_dry(d, q));
                // M6: 湿レイヤーを別経路で書き替えたので水彩の 1 段 undo を無効化
                self.invalidate_wet_undo();
            }
            if ui
                .button("にじみを止める")
                .on_hover_text("Fast Dry: 色はそのまま、水の動きだけ止める(固定はしない)。浮遊顔料をその場で沈着させ、流れをゼロに")
                .clicked()
            {
                self.run_canvas_action(|c, d, q| c.fast_dry(d, q));
                self.invalidate_wet_undo();
            }
            if ui
                .button("全体を濡らす")
                .on_hover_text("Wet the Layer: キャンバス全面を濡らす(水量は開発モードの「再湿潤の水量」)")
                .clicked()
            {
                self.run_canvas_action(|c, d, q| c.rewet(d, q));
                self.invalidate_wet_undo();
            }
        });
        // M6: 統一 Undo/Redo(水彩=1段 / 線画=多段)。乾燥ボタンと同じく常時見える位置に置く。
        // Ctrl+Z / Ctrl+Shift+Z と同じ経路(末尾の操作種別で振り分け)
        ui.horizontal(|ui| {
            let can_undo = !self.undo_stack.is_empty();
            let can_redo = !self.line_history.redo.is_empty();
            if ui
                .add_enabled(can_undo, egui::Button::new("↶ 元に戻す"))
                .on_hover_text("直前の操作を戻す (Ctrl+Z)。水彩は1段、線画は多段")
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("↷ やり直し"))
                .on_hover_text("取り消しをやり直す (Ctrl+Shift+Z)。対象は線画")
                .clicked()
            {
                self.redo();
            }
        });
        // F11: 制作者向け機能(味付け・診断・シミュ制御・記録再生・シェーダー状態)の表示切替。
        // off=通常ユーザー向け最小 UI。開発機能は削除でなくこのトグルの裏へ退避する
        ui.separator();
        ui.checkbox(&mut self.dev_mode, "🔧 開発モード")
            .on_hover_text(
                "味付けスライダー・診断表示・シミュ制御・ストローク記録再生・シェーダー状態を表示/退避する。\
                 通常の描画では off のままで OK",
            );
        ui.add_space(4.0);
    }

    /// 水彩レイヤーのツール(M1〜M4): ツール選択・顔料スロット・ブラシスライダー
    pub(in crate::app) fn brush_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("水彩ブラシ");
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
                    // レイヤーを離れて戻ったときの復元用(select_layer が読む)
                    self.last_wet_tool = wt;
                }
            }
        });
        if let Some(wt) = self.tool.wet() {
            self.params.tool = wt.gpu_id();
        }
        // F16: 選択中ツールの短い説明を常時1行(ホバー不要で「何をする筆か」が読める)
        if let Some(wt) = self.tool.wet() {
            ui.label(egui::RichText::new(wt.hint()).weak().small());
        }
        // 顔料セレクタ(M1c/M5): ブラシが注入する顔料スロット。色・名前・個性はランタイム
        // パレット(self.palette)から。編集はパレットパネル(palette_panel)で行う。
        // スロット情報を先にスナップショットしておく(ループ内で self.params を触るための借用回避)。
        // F2: ホバーは平易な日本語(数式記号は顔料の詳細設定側に残す)
        let swatches: Vec<(egui::Color32, String)> = self
            .palette
            .pigments
            .iter()
            .map(|p| {
                (
                    egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]),
                    format!(
                        "{}\n沈みやすさ {:.2} / 染みつき {:.2} / 粒状感 {:.2}",
                        p.name, p.density, p.staining, p.granulation
                    ),
                )
            })
            .collect();
        // F17: 選択中のスウォッチはアクセント色の太枠で明示(角丸のスウォッチ)
        ui.horizontal(|ui| {
            for (i, (color, hover)) in swatches.iter().enumerate() {
                let selected = self.params.brush_channel == i as u32;
                let mut button = egui::Button::new("")
                    .fill(*color)
                    .corner_radius(4.0)
                    .min_size(egui::vec2(28.0, 28.0));
                if selected {
                    button = button.stroke((3.0, ui.visuals().selection.stroke.color));
                }
                if ui.add(button).on_hover_text(hover.clone()).clicked() {
                    self.params.brush_channel = i as u32;
                }
            }
        });
        let pg = &self.palette.pigments[self.params.brush_channel.min(3) as usize];
        ui.label(pg.name.clone());
        // 共通スライダー(どのツールでも使う)
        ui.add(
            egui::Slider::new(&mut self.params.brush_radius, 1.0..=64.0)
                .text("太さ")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.brush_water, 0.0..=2.0).text("水量"));
        ui.add(egui::Slider::new(&mut self.params.brush_velocity, 0.0..=2.0).text("初速"));
        ui.add(egui::Slider::new(&mut self.params.brush_pigment, 0.0..=1.0).text("顔料量"));
        // F5: ツール固有スライダーは、そのツールを選んでいるときだけ出す(通常時の壁を減らす)
        match self.tool.wet() {
            Some(WetTool::Lift) => {
                ui.add(egui::Slider::new(&mut self.params.lift_strength, 0.0..=1.0).text("削りの強さ"));
            }
            Some(WetTool::WaterBrush) => {
                ui.add(egui::Slider::new(&mut self.params.water_lift, 0.0..=1.0).text("ぼかしの強さ"));
            }
            Some(WetTool::Smear) => {
                ui.add(
                    egui::Slider::new(&mut self.params.smear_rate, 0.0..=1.0)
                        .text("ならしの強さ(濃い所を伸ばす)"),
                );
            }
            _ => {}
        }
    }

    /// 線画レイヤー共通のヘッダ(M4.5a): 説明・消しゴムトグル。ツール状態を
    /// params.line_mode / line_eraser へ同期し linesplat.wgsl の分岐に渡す
    /// (描画先テクスチャの選択は CanvasCallback 経由)
    fn raster_tool_header(&mut self, ui: &mut egui::Ui, kind: RasterTool) {
        ui.label(egui::RichText::new(kind.hint()).weak());
        let mut eraser = matches!(self.tool, Tool::Raster { eraser: true, .. });
        ui.checkbox(&mut eraser, "消しゴム")
            .on_hover_text("このレイヤーの線を削る(splat を減算に反転)");
        self.tool = Tool::Raster { kind, eraser };
        self.params.line_mode = match kind {
            RasterTool::Pencil => 0,
            RasterTool::Pen => 1,
            RasterTool::Highlight => 2,
        };
        self.params.line_eraser = eraser as u32;
    }

    /// 下書き鉛筆レイヤーのツール(M4.5a)。太さ・濃さは水ブラシの brush_radius とは独立
    pub(in crate::app) fn pencil_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("下書き鉛筆");
        self.raster_tool_header(ui, RasterTool::Pencil);
        ui.add(
            egui::Slider::new(&mut self.params.pencil_radius, 1.0..=64.0)
                .text("太さ")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.pencil_strength, 0.0..=1.0).text("濃さ"));
        ui.add(egui::Slider::new(&mut self.params.pencil_gran, 0.0..=1.0).text("粒状感"));
        self.raster_tool_footer(ui);
    }

    /// 清書ペンレイヤーのツール(M4.5a/b)
    pub(in crate::app) fn pen_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("清書ペン");
        self.raster_tool_header(ui, RasterTool::Pen);
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
        self.raster_tool_footer(ui);
    }

    /// 白ハイライトレイヤーのツール(M4.5c)
    pub(in crate::app) fn highlight_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("ハイライト");
        self.raster_tool_header(ui, RasterTool::Highlight);
        ui.add(
            egui::Slider::new(&mut self.params.highlight_radius, 1.0..=64.0)
                .text("太さ")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.highlight_strength, 0.0..=1.0).text("不透明度"));
        self.raster_tool_footer(ui);
    }

    /// 線画レイヤー共通のフッタ: 筆圧の注記 + 多段 Undo/Redo(M4.5d)
    fn raster_tool_footer(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("※ペンの筆圧で濃さ・太さが変わります(効き具合は開発モードの「筆圧」で調整)")
                .weak()
                .small(),
        );
        self.line_history_controls(ui);
    }

    /// 乾燥レイヤー選択中の案内。焼き込みは一方通行なのでツールはなく、描画もブロックされる
    /// (canvas.rs の drawing_locked)。表示・順序の操作は右のレイヤーパネルで行う
    pub(in crate::app) fn dried_info_panel(&mut self, ui: &mut egui::Ui, index: usize) {
        let slot = {
            let renderer = self.render_state.renderer.read();
            renderer
                .callback_resources
                .get::<GpuCanvas>()
                .and_then(|c| c.layers.get(index))
                .map(|l| l.slot + 1)
        };
        match slot {
            Some(slot) => ui.heading(format!("乾いた層 {slot}")),
            None => ui.heading("乾いた層"),
        };
        ui.label("乾いて固定されたため編集できません(表示・順序は右のレイヤーパネルで)。");
        ui.label(
            egui::RichText::new(
                "乾いた層の再編集は設計上ありません(乾燥=一方通行)。いじり続けたい層は「乾かす(固定)」の代わりに「にじみを止める」で止めて、固定しない運用にしてください",
            )
            .weak(),
        );
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
