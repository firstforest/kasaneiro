//! 上部「ファイル」メニューバーと、そこから開くモーダル群(作品・設定プリセット・新規キャンバス・全部消す)。
//! 旧 save_panel / preset_panel(左パネル内インライン)を、画面上部メニュー + モーダル(egui::Modal)へ再構成した。
//! 破壊操作(新規キャンバス・全部消す)はモーダル内の明示ボタンで確認する(旧2度押し confirm_button を廃止)。

use crate::app::{FileModal, PaintApp};
use crate::palette_store;
use crate::pigment_store;
use crate::preset;
use crate::work;
use eframe::egui;
use paint_core::sim::CANVAS_SIZES;

/// 4色見本チップ(パレットの現行プレビュー・一覧行用)。クリック不可の小さな色角丸
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
    /// 上端の「ファイル」メニューバー(1メニュー)。項目クリックで menu_button は自動で閉じる
    /// (既定 PopupCloseBehavior=CloseOnClick)ので明示 close は不要。破壊操作はモーダル側で確認する
    pub(in crate::app) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("ファイル", |ui| {
                // 新規キャンバス(破壊操作): コンボの初期表示を現在サイズに合わせてからモーダルへ
                if ui.button("新規キャンバス…").clicked() {
                    self.pending_canvas_size = self.canvas_size;
                    self.file_modal = Some(FileModal::NewCanvas);
                }
                // 作品(保存・開く)は同じ統合モーダル(上段=保存・下段=一覧)。中身が同じなので
                // メニュー項目は1つに統合し、開くとき用に一覧を最新化してから開く
                if ui.button("作品…").clicked() {
                    self.work_ui.store.list = work::list();
                    self.file_modal = Some(FileModal::Work);
                }
                // パレット(4色一式)・色ライブラリ(1色)も名前付き保存/読込(M5f/g)。
                // 左パネルの「色をつくる」内のボタンからも同じモーダルが開く
                if ui.button("パレット…").clicked() {
                    self.open_palette_modal();
                }
                if ui.button("色ライブラリ…").clicked() {
                    self.open_pigment_modal();
                }
                // 設定プリセット(SimParams)も名前付き保存/読込。開く前に一覧を最新化
                if ui.button("設定プリセット…").clicked() {
                    self.preset_ui.store.list = preset::list();
                    self.file_modal = Some(FileModal::Preset);
                }
                ui.separator();
                // PNG 書き出しは非破壊なので確認なしで即実行(status_msg は save_snapshot 内で設定)
                if ui.button("画像を書き出す (PNG)").clicked() {
                    self.save_snapshot();
                }
                ui.separator();
                // 全部消す(破壊操作): モーダル内で確認
                if ui.button("全部消す…").clicked() {
                    self.file_modal = Some(FileModal::Clear);
                }
            });
        });
    }

    /// self.file_modal に応じて該当する1枚だけを描画する。背景クリック/Esc/[キャンセル] は
    /// いずれも「実行せず閉じる」=安全側。ctx は呼び出し側で clone 済み(借用回避)
    pub(in crate::app) fn file_modals(&mut self, ctx: &egui::Context) {
        match self.file_modal {
            Some(FileModal::Work) => self.work_modal(ctx),
            Some(FileModal::Preset) => self.preset_modal(ctx),
            Some(FileModal::Palette) => self.palette_modal(ctx),
            Some(FileModal::Pigment) => self.pigment_modal(ctx),
            Some(FileModal::NewCanvas) => self.new_canvas_modal(ctx),
            Some(FileModal::Clear) => self.clear_modal(ctx),
            None => {}
        }
    }

    /// パレットモーダルを開く(M5g)。一覧キャッシュを最新化してから開く
    /// (メニューと左パネル「色をつくる」の両方から呼ばれる)
    pub(in crate::app) fn open_palette_modal(&mut self) {
        self.palette_ui.store.list = palette_store::list();
        self.palette_ui.palette_cache = palette_store::load_all();
        self.file_modal = Some(FileModal::Palette);
    }

    /// 色ライブラリモーダルを開く(M5f)。一覧キャッシュを最新化してから開く
    pub(in crate::app) fn open_pigment_modal(&mut self) {
        self.palette_ui.pigment_cache = pigment_store::load_all();
        self.file_modal = Some(FileModal::Pigment);
    }

    /// 作品モーダル(統合): 上段=名前付き保存、下段=保存済み一覧から読込。
    /// 旧 work_panel の中身をほぼそのまま移設(save_work/load_work は &mut self なので直書き)
    fn work_modal(&mut self, ctx: &egui::Context) {
        let response = egui::Modal::new(egui::Id::new("file_modal_work")).show(ctx, |ui| {
            ui.heading("作品");

            // 上段=保存: 名前欄+[保存](名前空なら無効)+↻(一覧再読込)
            let mut do_save = None;
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.work_ui.store.name)
                        .hint_text("作品名")
                        .desired_width(200.0),
                );
                let name = self.work_ui.store.name.trim().to_owned();
                if ui
                    .add_enabled(!name.is_empty(), egui::Button::new("保存"))
                    .on_hover_text("描きかけの全状態(湿レイヤー・乾燥レイヤー・線画・パレット)を1ファイルに保存")
                    .clicked()
                {
                    do_save = Some(name);
                }
                if ui.button("↻").on_hover_text("一覧を再読込").clicked() {
                    self.work_ui.store.list = work::list();
                }
            });
            if let Some(name) = do_save {
                self.status_msg = Some(match self.save_work(&name) {
                    Ok(path) => {
                        self.work_ui.store.list = work::list();
                        format!("保存: {}", path.display())
                    }
                    Err(e) => e,
                });
            }

            ui.separator();

            // 下段=開く: 一覧から[読込]。読込後はモーダルを閉じる(status は load_work が設定)
            ui.label("保存済みの作品");
            let mut do_load = None;
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                if self.work_ui.store.list.is_empty() {
                    ui.label(egui::RichText::new("保存された作品はありません").weak());
                } else {
                    do_load = self.work_ui.store.list_rows(ui, "読込");
                }
            });
            if let Some(name) = do_load {
                self.load_work(&name);
                self.file_modal = None;
            }

            ui.separator();
            // 入力名は work_ui に残るので、閉じても保持される
            if ui.button("閉じる").clicked() {
                self.file_modal = None;
            }
        });
        if response.should_close() {
            self.file_modal = None;
        }
    }

    /// 設定プリセット モーダル(統合): 上段=名前付き保存、下段=保存済み一覧から読込。
    /// 旧 preset_panel の中身を移設(preset::save は params 値だけ要るので save_controls の closure に載る)
    fn preset_modal(&mut self, ctx: &egui::Context) {
        let response = egui::Modal::new(egui::Id::new("file_modal_preset")).show(ctx, |ui| {
            ui.heading("設定プリセット");
            ui.label(
                egui::RichText::new(
                    "味付けスライダーなどのパラメータ(SimParams)一式を名前を付けて保存/読込します",
                )
                .weak()
                .small(),
            );

            // 上段=保存: 名前欄+[保存]+↻(共通 NamedStore。preset::save は &self.params のみ要る)
            let params = self.params;
            if let Some(status) = self.preset_ui.store.save_controls(
                ui,
                "プリセット名",
                |name| preset::save(name, &params),
                preset::list,
            ) {
                self.status_msg = Some(status);
            }

            ui.separator();

            // 下段=読込: 一覧から[読込]。読込後はモーダルを閉じる
            ui.label("保存済みプリセット");
            let mut do_load = None;
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                if self.preset_ui.store.list.is_empty() {
                    ui.label(egui::RichText::new("保存されたプリセットはありません").weak());
                } else {
                    do_load = self.preset_ui.store.list_rows(ui, "読込");
                }
            });
            if let Some(name) = do_load {
                match preset::load(&name) {
                    Ok(params) => {
                        self.params = params;
                        self.preset_ui.store.name = name;
                        self.file_modal = None;
                    }
                    Err(e) => self.status_msg = Some(e),
                }
            }

            ui.separator();
            if ui.button("閉じる").clicked() {
                self.file_modal = None;
            }
        });
        if response.should_close() {
            self.file_modal = None;
        }
    }

    /// パレット モーダル(統合。M5g): 上段=現行4色プレビュー+名前付き保存、
    /// 下段=4色見本チップ付き一覧から読込。旧パレットパネル内インライン UI
    /// (palette.rs の保存/読込・既定に戻す)の移設先
    fn palette_modal(&mut self, ctx: &egui::Context) {
        let response = egui::Modal::new(egui::Id::new("file_modal_palette")).show(ctx, |ui| {
            ui.heading("パレット");
            ui.label(
                egui::RichText::new(
                    "いまの4色一式に名前を付けて保存/読込します。読込しても乾いた層の色は変わりません",
                )
                .weak()
                .small(),
            );

            // 上段=保存: 現行4色プレビュー+名前欄+[保存]+↻(共通 NamedStore)
            ui.horizontal(|ui| {
                palette_chips(ui, &self.palette);
                ui.label(egui::RichText::new("いまの4色").weak().small());
            });
            let palette = self.palette.clone();
            if let Some(status) = self.palette_ui.store.save_controls(
                ui,
                "パレット名",
                |name| palette_store::save(name, &palette),
                palette_store::list,
            ) {
                // 保存(成功・失敗どちらでも)後は一覧チップも最新化する
                self.palette_ui.palette_cache = palette_store::load_all();
                self.status_msg = Some(status);
            }

            ui.separator();

            // 下段=読込: 4色チップ付き一覧から[読込]。読込後はモーダルを閉じる
            ui.label("保存済みパレット");
            let mut do_load: Option<(String, pigment::Palette)> = None;
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                if self.palette_ui.palette_cache.is_empty() {
                    ui.label(egui::RichText::new("保存されたパレットはありません").weak());
                }
                for (name, pal) in &self.palette_ui.palette_cache {
                    ui.horizontal(|ui| {
                        if ui.button("読込").clicked() {
                            do_load = Some((name.clone(), pal.clone()));
                        }
                        palette_chips(ui, pal);
                        ui.label(name);
                    });
                }
            });
            if let Some((name, pal)) = do_load {
                self.palette = pal;
                self.apply_palette();
                self.status_msg = Some(format!("パレット読込: {name}"));
                self.file_modal = None;
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button("既定のパレットに戻す")
                    .on_hover_text("4スロットを起動時の顔料に戻す")
                    .clicked()
                {
                    self.palette = pigment::Palette::default_palette();
                    self.apply_palette();
                    self.status_msg = Some("既定のパレットに戻しました".to_owned());
                }
                if ui.button("閉じる").clicked() {
                    self.file_modal = None;
                }
            });
        });
        if response.should_close() {
            self.file_modal = None;
        }
    }

    /// 色ライブラリ モーダル(統合。M5f): 顔料1個の保存/読込。
    /// 上段=読み込み先スロットのミニセレクタ+選択スロットの即保存(名前欄なし=顔料名が保存名)、
    /// 下段=色見本+性質ホバー付き一覧から選択スロットへ[読込]
    fn pigment_modal(&mut self, ctx: &egui::Context) {
        let response = egui::Modal::new(egui::Id::new("file_modal_pigment")).show(ctx, |ui| {
            ui.heading("色ライブラリ");
            ui.label(
                egui::RichText::new(
                    "1色(名前・色・性質)を単体で保存/読込します。読込は選択中スロットへの上書きです",
                )
                .weak()
                .small(),
            );

            // 読み込み先スロット: ツールバーの色スウォッチと同じ意味論(brush_channel を変更)。
            // モーダルを閉じた後も選択スロットが同期しているのは仕様(閉じて選び直す往復をなくす)
            ui.horizontal(|ui| {
                ui.label("読み込み先スロット:");
                for i in 0..pigment::PIGMENT_COUNT {
                    let p = &self.palette.pigments[i];
                    let selected = self.params.brush_channel.min(3) as usize == i;
                    let mut button = egui::Button::new("")
                        .fill(egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]))
                        .corner_radius(3.0)
                        .min_size(egui::vec2(22.0, 22.0));
                    if selected {
                        button = button.stroke((2.0, ui.visuals().selection.stroke.color));
                    }
                    if ui
                        .add(button)
                        .on_hover_text(format!("#{} {}(ツールバーの色選びと連動)", i + 1, p.name))
                        .clicked()
                    {
                        self.params.brush_channel = i as u32;
                    }
                }
            });

            // 上段=保存: 選択スロットの色をその名前で即保存(保存名=顔料名の1本化なので名前欄なし)
            let ch = self.params.brush_channel.min(3) as usize;
            let current = self.palette.pigments[ch].clone();
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    3.0,
                    egui::Color32::from_rgb(current.rgb[0], current.rgb[1], current.rgb[2]),
                );
                ui.label(&current.name);
                let name = current.name.trim().to_owned();
                if ui
                    .add_enabled(!name.is_empty(), egui::Button::new("この色をとっておく"))
                    .on_hover_text(
                        "選択中スロットの色を、その名前でライブラリへ保存します(同名は上書き)。\n名前は左パネルの「色をつくる」で変えられます",
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
                if ui.button("↻").on_hover_text("一覧を再読込").clicked() {
                    self.palette_ui.pigment_cache = pigment_store::load_all();
                }
            });

            ui.separator();

            // 下段=読込: 色見本+名前(ホバーで性質)から選択スロットへ。読込後はモーダルを閉じる
            ui.label("とっておいた色");
            let mut do_load: Option<(String, pigment::Pigment)> = None;
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                if self.palette_ui.pigment_cache.is_empty() {
                    ui.label(egui::RichText::new("保存された色はありません").weak());
                }
                for (name, p) in &self.palette_ui.pigment_cache {
                    ui.horizontal(|ui| {
                        if ui.button("読込").clicked() {
                            do_load = Some((name.clone(), p.clone()));
                        }
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(24.0, 18.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            rect,
                            3.0,
                            egui::Color32::from_rgb(p.rgb[0], p.rgb[1], p.rgb[2]),
                        );
                        ui.label(name).on_hover_text(format!(
                            "沈みやすさ {:.2} / 染みつき {:.2} / 粒状感 {:.2}",
                            p.density, p.staining, p.granulation
                        ));
                    });
                }
            });
            if let Some((name, p)) = do_load {
                self.palette.pigments[ch] = p;
                self.apply_palette();
                self.status_msg = Some(format!("スロット #{} ← {name}", ch + 1));
                self.file_modal = None;
            }

            ui.separator();
            if ui.button("閉じる").clicked() {
                self.file_modal = None;
            }
        });
        if response.should_close() {
            self.file_modal = None;
        }
    }

    /// 新規キャンバス モーダル(破壊・確認込み): サイズ選択+[作成する]/[キャンセル]。
    /// 旧 save_panel の ComboBox + confirm_button(NewCanvas)を明示ボタンへ移植した
    fn new_canvas_modal(&mut self, ctx: &egui::Context) {
        let response = egui::Modal::new(egui::Id::new("file_modal_new_canvas")).show(ctx, |ui| {
            ui.heading("新規キャンバス");
            ui.label(
                egui::RichText::new(
                    "現在のキャンバスを破棄して選択サイズで作り直します。\
                     保存していない絵は消えます(残すなら先に作品保存を)",
                )
                .weak()
                .small(),
            );
            if self.pending_canvas_size != self.canvas_size {
                ui.label(
                    egui::RichText::new(format!("現在 {0}×{0}", self.canvas_size))
                        .weak()
                        .small(),
                );
            }
            egui::ComboBox::from_id_salt("new_canvas_size")
                .selected_text(format!("{0}×{0}", self.pending_canvas_size))
                .show_ui(ui, |ui| {
                    for s in CANVAS_SIZES {
                        ui.selectable_value(&mut self.pending_canvas_size, s, format!("{s}×{s}"));
                    }
                });
            ui.horizontal(|ui| {
                if ui.button("作成する").clicked() {
                    let size = self.pending_canvas_size;
                    self.recreate_canvas(size);
                    self.status_msg = Some(format!("新規キャンバス: {size}×{size}"));
                    self.file_modal = None;
                }
                if ui.button("キャンセル").clicked() {
                    self.file_modal = None;
                }
            });
        });
        if response.should_close() {
            self.file_modal = None;
        }
    }

    /// 全部消す モーダル(破壊・確認): 赤い[はい、全部消す]/[キャンセル]。
    /// 旧 save_panel の confirm_button(ClearCanvas)を明示ボタンへ移植した
    fn clear_modal(&mut self, ctx: &egui::Context) {
        let response = egui::Modal::new(egui::Id::new("file_modal_clear")).show(ctx, |ui| {
            ui.heading("全部消す");
            ui.label(
                egui::RichText::new(
                    "キャンバスを空に戻します。元に戻すで復帰できません(残すなら先に作品保存を)",
                )
                .weak()
                .small(),
            );
            ui.horizontal(|ui| {
                let clear_button = egui::Button::new(
                    egui::RichText::new("はい、全部消す").color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(200, 60, 60));
                if ui.add(clear_button).clicked() {
                    self.clear_canvas();
                    self.status_msg = Some("全部消しました".to_owned());
                    self.file_modal = None;
                }
                if ui.button("キャンセル").clicked() {
                    self.file_modal = None;
                }
            });
        });
        if response.should_close() {
            self.file_modal = None;
        }
    }
}
