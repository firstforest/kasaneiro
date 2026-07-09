//! 上部「ファイル」メニューバーと、そこから開くモーダル群(作品・設定プリセット・新規キャンバス・全部消す)。
//! 旧 save_panel / preset_panel(左パネル内インライン)を、画面上部メニュー + モーダル(egui::Modal)へ再構成した。
//! 破壊操作(新規キャンバス・全部消す)はモーダル内の明示ボタンで確認する(旧2度押し confirm_button を廃止)。

use crate::app::{FileModal, PaintApp};
use crate::preset;
use crate::work;
use eframe::egui;
use paint_core::sim::CANVAS_SIZES;

impl PaintApp {
    /// 上端のメニューバー(ファイル+ヘルプ)。項目クリックで menu_button は自動で閉じる
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
                // パレット(4色一式)・色ライブラリ(1色)はメイン機能なので左パネルに常時表示
                // (palette.rs。旧モーダルは廃止)。メニューにはファイル操作だけを残す。
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
            ui.menu_button("ヘルプ", |ui| {
                // バージョンとライセンス表示。exe 単体配布なのでライセンス文の同梱先はここ
                if ui.button("かさねいろについて…").clicked() {
                    self.file_modal = Some(FileModal::About);
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
            Some(FileModal::NewCanvas) => self.new_canvas_modal(ctx),
            Some(FileModal::Clear) => self.clear_modal(ctx),
            Some(FileModal::About) => self.about_modal(ctx),
            None => {}
        }
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

    /// かさねいろについて: バージョン+ライセンス表示。
    /// exe 単体配布(embed-assets)のためライセンス文の「同梱」はこの画面が正典:
    /// mixbox のクレジット(CC BY-NC 4.0 の表示義務)、M PLUS 1p の OFL 全文
    /// (フォントは include_bytes 埋め込みなのでファイル同梱されない)、依存クレート一覧。
    /// クレート一覧は assets/licenses/third-party.txt(git 管理)を include_str で埋め込む。
    /// 再生成コマンドは同ファイル冒頭のコメント参照(依存を追加・更新したら更新する)
    fn about_modal(&mut self, ctx: &egui::Context) {
        let response = egui::Modal::new(egui::Id::new("file_modal_about")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.heading(format!("かさねいろ v{}", env!("CARGO_PKG_VERSION")));
            ui.label(
                egui::RichText::new("水彩シミュレーションペイントツール")
                    .weak()
                    .small(),
            );
            ui.separator();

            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                ui.label(egui::RichText::new("混色エンジン").strong());
                ui.label("Mixbox © Secret Weapons — CC BY-NC 4.0(非商用)ライセンスで使用");
                ui.horizontal(|ui| {
                    ui.hyperlink_to("Mixbox", "https://scrtwpns.com/mixbox/");
                    ui.hyperlink_to(
                        "CC BY-NC 4.0",
                        "https://creativecommons.org/licenses/by-nc/4.0/",
                    );
                });
                ui.label(
                    egui::RichText::new("このため、かさねいろ自体も非商用利用に限られます")
                        .weak()
                        .small(),
                );
                ui.add_space(8.0);

                ui.label(egui::RichText::new("UI フォント").strong());
                ui.label("M PLUS 1p © The M+ FONTS Project — SIL Open Font License 1.1");
                egui::CollapsingHeader::new("OFL 1.1 全文")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(include_str!("../../../assets/fonts/OFL.txt"))
                                .weak()
                                .small(),
                        );
                    });
                ui.add_space(8.0);

                ui.label(egui::RichText::new("依存ライブラリ").strong());
                egui::CollapsingHeader::new("Rust クレート一覧(名前 — ライセンス)")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(include_str!(
                                "../../../assets/licenses/third-party.txt"
                            ))
                            .weak()
                            .small(),
                        );
                    });
            });

            ui.separator();
            if ui.button("閉じる").clicked() {
                self.file_modal = None;
            }
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
