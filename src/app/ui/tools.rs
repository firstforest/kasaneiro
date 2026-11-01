//! 乾燥ボタン(常時表示)と、アクティブレイヤーごとのツールパネル。
//!
//! レイヤーごとに使うツールが決まっているため、左パネルのツール群は右のレイヤーパネルで
//! 選択中のレイヤーに関係するものだけを出す([`PaintApp::active_tools_panel`] が出し分ける):
//! 水彩 → 水ブラシ+パレット / 鉛筆・ペン・ハイライト → 各線画ツール / 乾燥 → 編集不可の案内。

use crate::app::{ActiveLayer, PaintApp};
use crate::gpu::{DriedLayer, GpuCanvas};
use eframe::egui;
use paint_core::load::{self, BrushLoad};
use paint_core::tool::{RasterTool, Tool, ToolInfo, WetTool};

impl PaintApp {
    /// 選択中レイヤーのツールパネル(左パネル先頭)。右のレイヤーパネルの選択で出し分ける。
    /// F12: 「今のツール」のブロックを Frame::group で囲み、下の設定セクションと視覚的に分離する
    pub(in crate::app) fn active_tools_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| match self.active_layer {
            ActiveLayer::Wet => {
                self.brush_panel(ui);
                self.palette_panel(ui);
            }
            ActiveLayer::Pencil => self.pencil_panel(ui),
            ActiveLayer::Pen => self.pen_panel(ui),
            ActiveLayer::Highlight => self.highlight_panel(ui),
            ActiveLayer::Dried(index) => self.dried_info_panel(ui, index),
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
                // M6: 湿レイヤーを別経路で書き替えたら水彩の 1 段 undo を無効化
                if self.run_canvas_action(move |c, d, q| c.bake_dry(d, q, &pal)) {
                    self.invalidate_wet_undo();
                }
            }
            if ui
                .button("にじみを止める")
                .on_hover_text("Fast Dry: 色はそのまま、水の動きだけ止める(固定はしない)。浮遊顔料をその場で沈着させ、流れをゼロに")
                .clicked()
                && self.run_canvas_action(|c, d, q| c.fast_dry(d, q))
            {
                self.invalidate_wet_undo();
            }
            if ui
                .button("全体を濡らす")
                .on_hover_text("Wet the Layer: キャンバス全面を濡らす(水量は開発モードの「再湿潤の水量」)")
                .clicked()
                && self.run_canvas_action(|c, d, q| c.rewet(d, q))
            {
                self.invalidate_wet_undo();
            }
        });
        // M6: 統一 Undo/Redo(水彩=1段 / 線画=多段)。乾燥ボタンと同じく常時見える位置に置く。
        // Ctrl+Z / Ctrl+Shift+Z と同じ経路(末尾の操作種別で振り分け)
        ui.horizontal(|ui| {
            let can_undo = !self.painting && !self.undo_stack.is_empty();
            let can_redo = !self.painting && !self.line_history.redo.is_empty();
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

    /// 水彩レイヤーのツール(M1〜M4): ツール選択・筆の中身・ブラシスライダー
    pub(in crate::app) fn brush_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("水彩ブラシ");
        // 1列(折り返しあり)に「塗る」の色4つ+水+洗う、続けて削り/消す/吸い取り。
        // 色・水・洗うは筆の中身を作る操作で、押すと塗るツールにもなる。
        // 選択状態: 色は塗るツールのとき筆に入っている色、他の4ツールは tool==Wet(wt)。
        // F2: ホバーは平易な日本語(数式記号は顔料の詳細設定側に残す)
        let swatches: Vec<(egui::Color32, String)> = self
            .palette
            .pigments
            .iter()
            .map(|p| {
                (
                    egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]),
                    format!(
                        "{}を筆に取る(押している間、取り続けて濃くなる。筆の中で今の色と混ざる)\n\
                         沈みやすさ {:.2} / 染みつき {:.2} / 粒状感 {:.2} / 粒の細かさ {:.2}\n\
                         ダブルクリック: 色をつくる / 右クリック: ライブラリ保存",
                        p.name, p.density, p.staining, p.granulation, p.mobility
                    ),
                )
            })
            .collect();
        let dt = ui.input(|i| i.stable_dt);
        let now = ui.input(|i| i.time);
        let painting_tool = self.tool == Tool::Wet(WetTool::Paint);
        ui.horizontal_wrapped(|ui| {
            // 色スウォッチ: 押した瞬間に1回分、押している間も取り続ける。
            // F17: 筆に入っている色はアクセント色の太枠で明示(角丸のスウォッチ)
            for (i, (color, hover)) in swatches.iter().enumerate() {
                let selected = painting_tool && self.brush_load.pigment[i] > 0.0;
                let mut button = egui::Button::new("")
                    .fill(*color)
                    .corner_radius(4.0)
                    .min_size(egui::vec2(28.0, 28.0));
                if selected {
                    button = button.stroke((3.0, ui.visuals().selection.stroke.color));
                }
                let resp = ui.add(button).on_hover_text(hover.clone());
                // 文字のないボタンなので、支援技術向けに「塗る色 N: 顔料名」の名前を付ける
                // (e2e テストもこの名前でスウォッチを引く)
                let name = format!("塗る色 {}: {}", i + 1, self.palette.pigments[i].name);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
                match self.swatch_press(i, resp.is_pointer_button_down_on()) {
                    Some(true) => {
                        // ダブルクリックの1回目より前の中身を覚えておく(2回目の押下では上書きしない)
                        let fresh = !matches!(self.swatch_undo, Some((k, t, _)) if k == i && now - t < 0.5);
                        if fresh {
                            self.swatch_undo = Some((i, now, self.brush_load));
                        }
                        self.brush_load.take_pigment(i as u32, load::TAKE_DOSE);
                        self.after_brush_take();
                        self.params.brush_channel = i as u32;
                    }
                    Some(false) => {
                        self.brush_load.take_pigment(i as u32, load::TAKE_RATE * dt);
                        self.after_brush_take();
                    }
                    None => {}
                }
                // ダブルクリック: この色の「色をつくる」をモーダルで開く。開くための2回の押下で
                // 筆に取った色は取り消す(1回目の押下でスロットは選択済みなので、その色が編集対象になる)
                if resp.double_clicked() {
                    if let Some((k, _, before)) = self.swatch_undo.take()
                        && k == i
                    {
                        self.brush_load = before;
                        self.brush_refill = before;
                    }
                    self.palette_ui.color_modal = true;
                }
                // 右クリック: ライブラリ保存(色の管理の近道)
                resp.context_menu(|ui| {
                    let pname = self.palette.pigments[i].name.trim().to_owned();
                    ui.label(egui::RichText::new(self.palette.pigments[i].name.as_str()).strong());
                    if ui
                        .add_enabled(
                            !pname.is_empty(),
                            egui::Button::new("この色をとっておく(ライブラリへ)"),
                        )
                        .on_hover_text("名前・色・性質を色ライブラリへ保存します(同名は上書き)")
                        .clicked()
                    {
                        self.save_pigment_to_library(i);
                    }
                });
            }
            // 水: 押した瞬間に顔料が半分、押している間も水と入れ替わって抜けていく(長く押せば洗える)
            let water = egui::Button::new(egui::RichText::new("水").color(egui::Color32::from_rgb(40, 80, 140)))
                .fill(egui::Color32::from_rgb(200, 225, 245))
                .corner_radius(4.0)
                .min_size(egui::vec2(28.0, 28.0));
            let resp = ui.add(water).on_hover_text(
                "筆を水に浸す: 色が薄まる。押している間、色が水と入れ替わって抜けていき、\
                 長く押すと透明な水だけの筆になる",
            );
            match self.swatch_press(4, resp.is_pointer_button_down_on()) {
                Some(true) => {
                    self.brush_load.dip_water(load::DIP_KEEP);
                    self.after_brush_take();
                }
                Some(false) => {
                    self.brush_load.dip_water(BrushLoad::dip_keep(dt));
                    self.after_brush_take();
                }
                None => {}
            }
            if ui
                .add(egui::Button::new("洗う").min_size(egui::vec2(0.0, 28.0)))
                .on_hover_text("筆を洗う: 水も色もすべて落として空の筆にする(リセット)。続けて「水」を押すと水だけの筆になる")
                .clicked()
            {
                self.brush_load.wash();
                self.after_brush_take();
            }
            // 「筆の中身を作る操作」と「削り以降の4ツール」の意味グループを小さいギャップで区切る
            // (horizontal_wrapped の折返し挙動を壊さないよう要素の追加はしない)
            ui.add_space(8.0);
            // 削り/消す/吸い取り: 枠付きの文字ボタン(Paint はスウォッチ側で出すので飛ばす)。
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
        // 塗るときは筆の中身(混ざった色・濃さ・残りの水)を出す
        if self.tool == Tool::Wet(WetTool::Paint) {
            self.brush_load_view(ui);
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
        // F5: ツール固有スライダーは、そのツールを選んでいるときだけ出す(通常時の壁を減らす)
        match self.tool.wet() {
            Some(WetTool::Paint) => {
                ui.checkbox(&mut self.auto_refill, "筆を下ろすたびに含み直す")
                    .on_hover_text(
                        "on: 最後に色・水・洗うで作った中身へ戻してから描き始める。\
                         off: 前のストロークの残りから続き、描くほど減っていく",
                    );
                ui.add(
                    egui::Slider::new(&mut self.params.brush_hold, 0.0..=5000.0)
                        .text("含みの持ち")
                        .suffix(" px"),
                )
                .on_hover_text(
                    "この長さをなぞると筆の中身(水と色)が半分になる。濃さは変わらず量が減る。0=減らない",
                );
                ui.add(
                    egui::Slider::new(&mut self.params.brush_charge, 0.0..=5.0)
                        .text("筆の含み")
                        .suffix(" 秒"),
                )
                .on_hover_text(
                    "筆を置いたまま動かさなくても色水が流れ出続ける。満タンの筆ならこの秒数で\
                     中身の水が尽きる(なぞり直さなくても水を伝って広がっていく)。\
                     広がる距離は「水量」と含みの長さで決まる。0=置いた瞬間だけ",
                );
                ui.add(
                    egui::Slider::new(&mut self.params.paint_pickup, 0.0..=1.0)
                        .text("下の色を溶かす"),
                )
                .on_hover_text(
                    "筆が触れた沈着顔料を浮かせて筆の色と混ぜる割合。水だけの筆で\
                     なでるときは、これがぼかしの強さになる。染みつき(ω)が強い色は剥がれず残る",
                );
            }
            Some(WetTool::Lift) => {
                ui.add(
                    egui::Slider::new(&mut self.params.lift_strength, 0.0..=1.0).text("削りの強さ"),
                )
                .on_hover_text("紙に付いた色を水で浮かせる割合。染みつき(ω)が強い色は残る");
                ui.add(
                    egui::Slider::new(&mut self.params.lift_blot, 0.0..=1.0).text("押さえて吸う"),
                )
                .on_hover_text(
                    "浮かせた色を水ごと吸い取る割合。上げるほどその場で白く抜ける。\
                     0 = 浮かせるだけ(色は周りへ移る)",
                );
            }
            Some(WetTool::Absorb) => {
                ui.add(
                    egui::Slider::new(&mut self.params.absorb_rate, 0.0..=1.0)
                        .text("吸い取りの強さ"),
                )
                .on_hover_text(
                    "ひとなでで水と浮いている色を取り除く割合。強く押すほど多く吸う。\
                     紙に沈着した色は残る(剥がしたいときは「削り」)",
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

    /// 線画レイヤー共通のフッタ: 筆圧の注記 + 線画履歴の本数(M4.5d)
    fn raster_tool_footer(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(
                "※ペンの筆圧で濃さ・太さが変わります(効き具合は開発モードの「筆圧」で調整)",
            )
            .weak()
            .small(),
        );
        self.line_history_status(ui);
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
            let recorded = canvas.and_then(|c| slot.and_then(|s| c.layer_palette(s).cloned()));
            (slot, recorded)
        };
        match slot {
            Some(slot) => ui.heading(format!("乾いた層 {}", slot + 1)),
            None => ui.heading("乾いた層"),
        };
        ui.label("乾いて固定されたため描き足せません(表示・順序は右のレイヤーパネル、濃さは下のスライダーで)。");
        self.dried_density_slider(ui, index);
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
                        "{}\n沈みやすさ {:.2} / 染みつき {:.2} / 粒状感 {:.2} / 粒の細かさ {:.2}\nクリックでこの1色をスロット #{} に取り込む",
                        p.name,
                        p.density,
                        p.staining,
                        p.granulation,
                        p.mobility,
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
                "この層を描いたときのパレット(名前・性質ごと)へ丸ごと切り替えます。\n今の4色が惜しければ先に水彩レイヤーの「パレット(4色一式)」で保存してください",
            )
            .clicked()
        {
            self.palette = rec.clone();
            self.apply_palette();
            self.status_msg = Some(format!("層{layer_no}のパレットを取り込みました"));
        }
    }

    /// 乾いた層の濃度倍率。合成時に顔料濃度へ掛けるだけなので、いつでも 1 に戻せる
    fn dried_density_slider(&mut self, ui: &mut egui::Ui, index: usize) {
        let mut renderer = self.render_state.renderer.write();
        let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() else {
            return;
        };
        let Some(layer) = canvas.layers.get_mut(index) else {
            return;
        };
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui
                .add(egui::Slider::new(&mut layer.density, DriedLayer::DENSITY_RANGE).text("濃さ"))
                .on_hover_text("この層の顔料の濃さを倍率で変える(1 = 乾かしたときのまま)。下地へ統合するとその値で固まる")
                .changed();
            if ui.add_enabled(layer.density != 1.0, egui::Button::new("1 に戻す")).clicked() {
                layer.density = 1.0;
                changed = true;
            }
        });
        if changed {
            canvas.sync_layers(&self.render_state.queue);
        }
    }

    /// 線画の多段 Undo/Redo(M4.5d)の履歴本数の表示。操作は左パネル最上部の統一
    /// 「元に戻す / やり直し」(dry_controls)と Ctrl+Z / Ctrl+Shift+Z が担う
    fn line_history_status(&self, ui: &mut egui::Ui) {
        ui.label(format!(
            "線画履歴: {} 本(やり直し {} 本)",
            self.line_history.done.len(),
            self.line_history.redo.len()
        ));
    }

    /// スウォッチ idx(0〜3 = 色、4 = 水)の押下状態を進める。
    /// Some(true) = 押した瞬間、Some(false) = 押し続けている、None = 押していない
    fn swatch_press(&mut self, idx: usize, down: bool) -> Option<bool> {
        if !down {
            if self.swatch_held == Some(idx) {
                self.swatch_held = None;
            }
            return None;
        }
        let first = self.swatch_held != Some(idx);
        self.swatch_held = Some(idx);
        Some(first)
    }

    /// 色・水・洗うで筆の中身を作った後: 塗るツールにし、自動補充の戻り先を今の中身にする
    fn after_brush_take(&mut self) {
        self.tool = Tool::Wet(WetTool::Paint);
        // レイヤーを離れて戻ったときの復元用(select_layer が読む)
        self.last_wet_tool = WetTool::Paint;
        self.brush_refill = self.brush_load;
    }

    /// 筆の状態パネル: 混ざった色の見本・濃さ・水のゲージ・顔料の色別積み上げゲージ。
    /// 描いて減る・取って増える様子を毎フレーム追従して見せる
    fn brush_load_view(&self, ui: &mut egui::Ui) {
        let load = self.brush_load;
        let paper = pigment::PAPER_RGB.map(|c| c * 255.0);
        let shown = match self.palette.mix_rgb(&load.pigment) {
            Some(rgb) => {
                // 見本の色の乗り方: 濃さ 1 で 7 割、2 で 9 割ほど
                let t = 1.0 - (-1.2 * load.concentration()).exp();
                let ch = |k: usize| (paper[k] + (rgb[k] as f32 - paper[k]) * t).round() as u8;
                egui::Color32::from_rgb(ch(0), ch(1), ch(2))
            }
            None => egui::Color32::from_rgb(paper[0] as u8, paper[1] as u8, paper[2] as u8),
        };
        let total = load.pigment_total();
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        let track = ui.visuals().extreme_bg_color;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 14.0, shown);
                ui.painter()
                    .rect_stroke(rect, 14.0, stroke, egui::StrokeKind::Inside);
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "筆の中身")
                });
                ui.vertical(|ui| {
                    ui.label("筆の状態");
                    ui.label(
                        egui::RichText::new(format!("濃さ ×{:.1}", load.concentration()))
                            .weak()
                            .small(),
                    );
                });
            });

            // 水: 0〜100%(満タン = 1)
            let water = load.water.clamp(0.0, 1.0);
            gauge_row(
                ui,
                "含む水",
                format!("{:.0}%", water * 100.0),
                stroke,
                track,
                |painter, r| {
                    let w = r.width() * water;
                    painter.rect_filled(
                        egui::Rect::from_min_size(r.min, egui::vec2(w, r.height())),
                        3.0,
                        egui::Color32::from_rgb(90, 150, 220),
                    );
                },
            );

            // 顔料: 色ごとの量を色つきで左から積み上げる(上限 PIGMENT_MAX 回分)
            gauge_row(
                ui,
                "含む顔料",
                if total <= 0.0 {
                    "なし".to_owned()
                } else {
                    format!("{total:.1}")
                },
                stroke,
                track,
                |painter, r| {
                    let mut x = r.min.x;
                    for (p, a) in self.palette.pigments.iter().zip(load.pigment) {
                        let w = r.width() * (a / load::PIGMENT_MAX).clamp(0.0, 1.0);
                        if w <= 0.0 {
                            continue;
                        }
                        let seg = egui::Rect::from_min_size(
                            egui::pos2(x, r.min.y),
                            egui::vec2(w.min(r.max.x - x), r.height()),
                        );
                        painter.rect_filled(
                            seg,
                            0.0,
                            egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]),
                        );
                        x += w;
                    }
                },
            );

            // 内訳(5% 以上の色だけ)
            if total > 0.0 {
                let parts = self
                    .palette
                    .pigments
                    .iter()
                    .zip(load.pigment)
                    .filter(|(_, a)| *a / total >= 0.05)
                    .map(|(p, a)| format!("{} {:.0}%", p.name, a / total * 100.0))
                    .collect::<Vec<_>>()
                    .join(" + ");
                ui.label(egui::RichText::new(parts).weak().small());
            }
        });
    }
}

/// ラベル + 横ゲージ + 数値の1行。draw は枠内(トラック上)に中身を描く
fn gauge_row(
    ui: &mut egui::Ui,
    label: &str,
    value: String,
    stroke: egui::Stroke,
    track: egui::Color32,
    draw: impl FnOnce(&egui::Painter, egui::Rect),
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [56.0, 14.0],
            egui::Label::new(egui::RichText::new(label).small()),
        );
        let width = (ui.available_width() - 48.0).max(40.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 12.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 3.0, track);
        draw(painter, rect);
        painter.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
        ui.label(egui::RichText::new(value).small());
    });
}
