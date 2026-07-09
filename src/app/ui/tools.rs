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
                // M5h: 焼き込み時点の現行パレットを渡して CPU 側にも記録する(抽出の正典)
                let pal = self.palette.clone();
                self.run_canvas_action(move |c, d, q| c.bake_dry(d, q, &pal));
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
                .add_enabled(can_undo, egui::Button::new("元に戻す"))
                .on_hover_text("直前の操作を戻す (Ctrl+Z)。水彩は1段、線画は多段")
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("やり直し"))
                .on_hover_text("取り消しをやり直す (Ctrl+Shift+Z)。対象は線画")
                .clicked()
            {
                self.redo();
            }
        });
    }

    /// F11: 制作者向け機能(味付け・診断・シミュ制御・記録再生・シェーダー状態)の表示切替。
    /// off=通常ユーザー向け最小 UI。開発機能は削除でなくこのトグルの裏へ退避する。
    /// 誤操作しにくいよう、左パネルの下端(左下)へ固定して置く(mod.rs の Panel::bottom)
    pub(in crate::app) fn dev_mode_toggle(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
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
        // 8ツールを1列(並列)に並べる: 「塗る」4色(色スウォッチ)+ 削り/消す/ぼかし筆/ならし
        // (文字ボタン)。**色選び = その色で「塗る」ツールを選ぶ**に統合したので、旧「ツール選択行 +
        // 別行の顔料スウォッチ」の2段はやめて 8 ボタンを一列にした(色スウォッチを押すと Paint ツール +
        // その顔料スロットへ切り替わる)。選択状態: 塗るは tool==Paint かつ brush_channel==i、
        // 他の4ツールは tool==Wet(wt)(GPU 値・ラベル・文言は enum の impl に一元化。TOOLS 定数表は廃止)。
        // F2: ホバーは平易な日本語(数式記号は顔料の詳細設定側に残す)
        let swatches: Vec<(egui::Color32, String)> = self
            .palette
            .pigments
            .iter()
            .map(|p| {
                (
                    egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]),
                    format!(
                        "{}で塗る\n沈みやすさ {:.2} / 染みつき {:.2} / 粒状感 {:.2}",
                        p.name, p.density, p.staining, p.granulation
                    ),
                )
            })
            .collect();
        // 左パネルは幅が狭いので横一列に収まらないぶんは折り返す(8 ボタン=色4+文字4)
        ui.horizontal_wrapped(|ui| {
            // 「塗る」4色: 色スウォッチ。押すと Paint ツール + その顔料スロットへ。
            // F17: 選択中のスウォッチはアクセント色の太枠で明示(角丸のスウォッチ)
            for (i, (color, hover)) in swatches.iter().enumerate() {
                let selected = self.tool == Tool::Wet(WetTool::Paint)
                    && self.params.brush_channel == i as u32;
                let mut button = egui::Button::new("")
                    .fill(*color)
                    .corner_radius(4.0)
                    .min_size(egui::vec2(28.0, 28.0));
                if selected {
                    button = button.stroke((3.0, ui.visuals().selection.stroke.color));
                }
                if ui.add(button).on_hover_text(hover.clone()).clicked() {
                    self.tool = Tool::Wet(WetTool::Paint);
                    self.params.brush_channel = i as u32;
                    // レイヤーを離れて戻ったときの復元用(select_layer が読む)
                    self.last_wet_tool = WetTool::Paint;
                }
            }
            // 「塗る4色」と「削り以降の4ツール」の意味グループを小さいギャップで区切る
            // (horizontal_wrapped の折返し挙動を壊さないよう要素の追加はしない)
            ui.add_space(8.0);
            // 削り/消す/ぼかし筆/ならし: 枠付きの文字ボタン(Paint はスウォッチ側で出すので飛ばす)。
            // 色スウォッチと高さ 28px を揃え、選択中は F17 のスウォッチ選択太枠と整合する
            // selection 色の塗り+枠で明示する
            for wt in WetTool::ALL {
                if wt == WetTool::Paint {
                    continue;
                }
                let selected = self.tool == Tool::Wet(wt);
                let mut button =
                    egui::Button::new(wt.label()).min_size(egui::vec2(0.0, 28.0));
                if selected {
                    button = button
                        .fill(ui.visuals().selection.bg_fill)
                        .stroke((3.0, ui.visuals().selection.stroke.color));
                }
                // 消すは E キー押しっぱなしでも一時的に使える(ペンタブのキー割当)ことをホバーで案内
                let hover = if wt == WetTool::Erase {
                    format!(
                        "{}\nE キーを押している間だけ一時的に消すにもできます(ペンタブレットのキーに E を割り当て)",
                        wt.hint()
                    )
                } else {
                    wt.hint().to_owned()
                };
                if ui.add(button).on_hover_text(hover).clicked() {
                    self.tool = Tool::Wet(wt);
                    self.last_wet_tool = wt;
                }
            }
        });
        if let Some(wt) = self.tool.wet() {
            self.params.tool = wt.gpu_id();
        }
        // スポイトは色選びの動線なのでツールバーの直後に置く(M5e。旧: パレットパネル内)
        self.eyedropper_control(ui);
        // F16: 選択中ツールの短い説明を常時1行(ホバー不要で「何をする筆か」が読める)。
        // 左パネル幅で折り返さないよう常時表示は short_hint、詳しい説明はホバーに温存
        if let Some(wt) = self.tool.wet() {
            ui.label(egui::RichText::new(wt.short_hint()).weak().small())
                .on_hover_text(wt.hint());
        }
        // 塗るときは選択中の顔料名を出す(削り等では顔料は無関係なので出さない)
        if self.tool == Tool::Wet(WetTool::Paint) {
            let pg = &self.palette.pigments[self.params.brush_channel.min(3) as usize];
            ui.label(format!("塗る色: {}", pg.name));
        }
        // 共通スライダー(どのツールでも使う)
        ui.add(
            egui::Slider::new(&mut self.params.brush_radius, 1.0..=64.0)
                .text("太さ")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.brush_water, 0.0..=2.0).text("水量"));
        ui.add(egui::Slider::new(&mut self.params.brush_velocity, 0.0..=2.0).text("筆の勢い"))
            .on_hover_text("初速: ストロークが水に与える初期速度。上げるほど置いた色が流れ出す");
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
            .on_hover_text(
                "このレイヤーの線を削る(splat を減算に反転)。\nE キーを押している間だけ一時的に消しゴムにもできます(ペンタブレットのキーに E を割り当て)",
            );
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
    /// (canvas.rs の drawing_locked)。表示・順序の操作は右のレイヤーパネルで行う。
    /// M5h: この層を描いたときのパレット(CPU 記録=layer_palettes)からの色取り込み UI を持つ
    pub(in crate::app) fn dried_info_panel(&mut self, ui: &mut egui::Ui, index: usize) {
        // slot 解決と記録時パレットの clone は renderer.read() の間に済ませる
        // (取り込みの apply_palette は renderer.write() を取るためロックを跨げない)
        let (slot, recorded) = {
            let renderer = self.render_state.renderer.read();
            let canvas = renderer.callback_resources.get::<GpuCanvas>();
            let slot = canvas
                .and_then(|c| c.layers.get(index))
                .map(|l| l.slot as usize);
            let recorded =
                canvas.and_then(|c| slot.and_then(|s| c.layer_palette(s).cloned()));
            (slot, recorded)
        };
        match slot {
            Some(slot) => ui.heading(format!("乾いた層 {}", slot + 1)),
            None => ui.heading("乾いた層"),
        };
        ui.label("乾いて固定されたため編集できません(表示・順序は右のレイヤーパネルで)。");
        ui.label(
            egui::RichText::new(
                "乾いた層の再編集は設計上ありません(乾燥=一方通行)。いじり続けたい層は「乾かす(固定)」の代わりに「にじみを止める」で止めて、固定しない運用にしてください",
            )
            .weak(),
        );

        // M5h: 記録時パレットからの取り込み。スウォッチクリック=1色だけ同番スロットへ、
        // ボタン=4色丸ごと。読込時正規化(load_work)により通常 recorded は常に Some だが、
        // 将来の変更(レイヤー個別削除など)で崩れたときの破綻検知を兼ねて None も明示する
        ui.separator();
        let Some(rec) = recorded else {
            ui.label(egui::RichText::new("この層のパレット記録はありません").weak());
            return;
        };
        let layer_no = slot.unwrap_or(0) + 1;
        ui.label("この層を描いたときの4色:");
        let mut pick = None;
        ui.horizontal(|ui| {
            for (i, p) in rec.pigments.iter().enumerate() {
                let button = egui::Button::new("")
                    .fill(egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]))
                    .corner_radius(4.0)
                    .min_size(egui::vec2(24.0, 24.0));
                if ui
                    .add(button)
                    .on_hover_text(format!(
                        "{}\n沈みやすさ {:.2} / 染みつき {:.2} / 粒状感 {:.2}\nクリックでこの1色をスロット #{} に取り込む",
                        p.name,
                        p.density,
                        p.staining,
                        p.granulation,
                        i + 1
                    ))
                    .clicked()
                {
                    pick = Some(i);
                }
            }
        });
        if let Some(i) = pick {
            self.palette.pigments[i] = rec.pigments[i].clone();
            self.apply_palette();
            self.status_msg = Some(format!("層{layer_no}の色{}を取り込みました", i + 1));
        }
        if ui
            .button("この4色をいまのパレットにする")
            .on_hover_text(
                "この層を描いたときのパレット(名前・性質ごと)へ丸ごと切り替えます。\n今の4色が惜しければ先に「パレット…」で保存してください",
            )
            .clicked()
        {
            self.palette = rec.clone();
            self.apply_palette();
            self.status_msg = Some(format!("層{layer_no}のパレットを取り込みました"));
        }
    }

    /// 線画の多段 Undo/Redo(M4.5d): ボタン+履歴本数の表示。キーは Ctrl+Z / Ctrl+Shift+Z。
    /// 湿レイヤー(水彩)は対象外(M6 の 1 段 undo で扱う)
    pub(in crate::app) fn line_history_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let can_undo = !self.line_history.done.is_empty();
            let can_redo = !self.line_history.redo.is_empty();
            if ui
                .add_enabled(can_undo, egui::Button::new("元に戻す"))
                .on_hover_text("線画を1本戻す (Ctrl+Z)。水彩は対象外")
                .clicked()
            {
                self.line_undo();
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("やり直し"))
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
