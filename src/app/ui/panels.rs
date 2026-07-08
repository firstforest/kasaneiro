//! プリセット(H3)・ストローク記録再生(H5)・シェーダー状態(H1)のパネル。
//! app/mod.rs から分割(R4)。

use crate::app::PaintApp;
use crate::gpu::hot_reload::shader_dir;
use crate::preset;
use crate::replay::{self, Recorder, Recording};
use crate::work;
use eframe::egui;

impl PaintApp {
    /// プリセット(H3): 名前保存+一覧読込。共通 UI は NamedStore に集約(R4)
    pub(in crate::app) fn preset_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("設定プリセット");
        let params = self.params;
        if let Some(status) =
            self.preset_ui
                .store
                .save_controls(ui, "プリセット名", |name| preset::save(name, &params), preset::list)
        {
            self.status_msg = Some(status);
        }
        if let Some(name) = self.preset_ui.store.list_rows(ui, "読込") {
            match preset::load(&name) {
                Ok(params) => {
                    self.params = params;
                    self.preset_ui.store.name = name;
                }
                Err(e) => self.status_msg = Some(e),
            }
        }
    }

    /// 作品保存(M7): 描きかけ(湿レイヤー含む)を1ファイルへ保存/読込。
    /// 保存は GPU 読み戻し(&mut self)が要るので NamedStore.save_controls の closure に載せられず、
    /// 名前欄+ボタンを直に描いてクリック時に self.save_work を呼ぶ(一覧の読込は list_rows を流用)
    pub(in crate::app) fn work_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("作品を保存");
        let mut do_save = None;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.work_ui.store.name)
                    .hint_text("作品名")
                    .desired_width(140.0),
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
        if let Some(name) = self.work_ui.store.list_rows(ui, "読込") {
            self.load_work(&name);
        }
    }

    /// 保存・書き出しセクション(F9): 作品保存/読込に、画像書き出し(PNG)・全消去・
    /// キャンバスサイズを集約する。どれも通常ユーザーが使う機能なので開発モードに関係なく常時表示。
    /// 旧「制御 (H6)」に混在していた PNG/リセット/サイズを、動線に沿ってここへ移した
    pub(in crate::app) fn save_panel(&mut self, ui: &mut egui::Ui) {
        self.work_panel(ui);

        ui.horizontal(|ui| {
            if ui
                .button("画像を書き出す (PNG)")
                .on_hover_text("いま見えている絵を PNG 画像として snapshots/ に書き出す")
                .clicked()
            {
                self.save_snapshot();
            }
            if ui
                .button("全部消す")
                .on_hover_text("キャンバスを空に戻す(元に戻すで復帰不可。残すなら先に作品保存を)")
                .clicked()
            {
                self.clear_canvas();
            }
        });

        // キャンバスサイズ(M8): 正方形 512/1024/2048。テクセル密度は据え置き=「広い紙」。
        // 変更は新規キャンバスの作り直しなので、描きかけは先に作品保存してから
        ui.horizontal(|ui| {
            ui.label("キャンバスサイズ");
            egui::ComboBox::from_id_salt("canvas_size")
                .selected_text(format!("{0}×{0}", self.pending_canvas_size))
                .show_ui(ui, |ui| {
                    for s in paint_core::sim::CANVAS_SIZES {
                        ui.selectable_value(&mut self.pending_canvas_size, s, format!("{s}×{s}"));
                    }
                });
            if ui
                .button("新規キャンバス")
                .on_hover_text(
                    "現在のキャンバスを破棄して選択サイズで作り直す(広い紙。テクセル密度は同じ)。\
                     保存していない絵は消えるので、残すなら先に作品保存を",
                )
                .clicked()
            {
                let size = self.pending_canvas_size;
                self.recreate_canvas(size);
                self.status_msg = Some(format!("新規キャンバス: {size}×{size}"));
            }
        });
        if self.pending_canvas_size != self.canvas_size {
            ui.label(
                egui::RichText::new(format!(
                    "現在 {0}×{0}(「新規キャンバス」で切り替え)",
                    self.canvas_size
                ))
                .weak()
                .small(),
            );
        }
    }

    /// 操作結果(保存先パス・スポイト・エラー)の1行表示(F3)。左パネルのスクロール外・
    /// 常時可視の位置で呼ぶので、開発モードの記録再生パネルに埋めず、どの操作でも同じ場所に出る
    pub(in crate::app) fn status_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(msg) = &self.status_msg {
            ui.separator();
            ui.label(msg.clone());
        }
    }

    /// ストローク記録・再生(H5): 記録操作・保存・一覧再生。名前保存+一覧は NamedStore(R4)
    pub(in crate::app) fn replay_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("ストローク記録・再生 (H5)");
        ui.horizontal(|ui| {
            match &self.replay_ui.recorder {
                None => {
                    if ui.button("⏺ 記録開始").clicked() {
                        self.replay_ui.recorder = Some(Recorder::new());
                        self.replay_ui.pending_recording = None;
                    }
                }
                Some(_) => {
                    if ui.button("⏹ 記録停止").clicked() {
                        let recording = self.replay_ui.recorder.take().unwrap().finish();
                        if recording.is_empty() {
                            self.status_msg = Some("ストロークが記録されていません".to_owned());
                        } else {
                            self.replay_ui.pending_recording = Some(recording);
                        }
                    }
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "記録中…");
                }
            }
            if self.replay_ui.player.is_some() {
                if ui.button("■ 再生停止").clicked() {
                    self.stop_replay();
                }
                ui.colored_label(egui::Color32::from_rgb(64, 160, 64), "再生中…");
            }
        });
        // 記録直後: 名前を付けて保存(共通 NamedStore)+ そのまま試し再生。
        // M5d: 保存には記録時のパレット(現行パレット)を添える。試し再生は現行パレットのまま
        let mut replay_now: Option<(Recording, Option<pigment::Palette>)> = None;
        if let Some(recording) = self.replay_ui.pending_recording.clone() {
            let palette = self.palette.clone();
            if let Some(status) = self.replay_ui.store.save_controls(
                ui,
                "ストローク名",
                |name| replay::save(name, &recording, Some(&palette)),
                replay::list,
            ) {
                self.status_msg = Some(status);
            }
            if ui.button("試し再生").clicked() {
                replay_now = Some((recording, None));
            }
        }
        if let Some(name) = self.replay_ui.store.list_rows(ui, "▶ 再生") {
            match replay::load(&name) {
                Ok(stored) => replay_now = Some((stored.recording, stored.palette)),
                Err(e) => self.status_msg = Some(e),
            }
        }
        if let Some((recording, palette)) = replay_now {
            self.start_replay(recording, palette);
        }
        // status_msg の表示は status_bar(常時可視・スクロール外)へ移した(F3)
    }

    /// ビュー(M6): 拡大率の表示・全体表示に戻す・操作ヒント。
    /// 拡大/パンの実操作はキャンバス上のホイール・中ボタンドラッグで行う(canvas.rs)
    pub(in crate::app) fn view_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("表示(ズーム・回転)");
        // 通常時はこの1行だけ: 拡大率と「全体表示に戻す」(拡大中に迷子から復帰する用)
        ui.horizontal(|ui| {
            ui.label(format!("拡大 {:.0}%", self.view_zoom * 100.0));
            let rotated = self.view_zoom > 1.0 || self.view_angle != 0.0;
            if ui
                .add_enabled(rotated, egui::Button::new("全体表示に戻す"))
                .clicked()
            {
                self.reset_view();
            }
        });
        // F13: 回転スライダー・スナップ・操作ヒントは詳細として畳む(実操作はキャンバス上の
        // ホイール/ドラッグで行うので、畳んでも体験は落ちない)
        egui::CollapsingHeader::new("表示の詳細")
            .default_open(false)
            .show(ui, |ui| {
                // 回転(表示中心まわり)。スライダーは自由角、ボタンは 15°スナップ
                ui.horizontal(|ui| {
                    ui.label("回転");
                    let mut deg = self.view_angle.to_degrees();
                    if ui
                        .add(egui::Slider::new(&mut deg, -180.0..=180.0).suffix("°"))
                        .changed()
                    {
                        self.view_angle = deg.to_radians();
                        self.clamp_view();
                    }
                    if ui.small_button("−15°").clicked() {
                        self.rotate_view(-std::f32::consts::FRAC_PI_8 * 1.5);
                    }
                    if ui.small_button("+15°").clicked() {
                        self.rotate_view(std::f32::consts::FRAC_PI_8 * 1.5);
                    }
                    if ui.small_button("0°").clicked() {
                        self.view_angle = 0.0;
                        self.clamp_view();
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "ホイール=拡大 / Shift+ホイール=15°回転 / 中ボタン・スペース+左ドラッグ=パン",
                    )
                    .weak()
                    .small(),
                );
            });
    }

    /// シェーダー(H1): 監視ディレクトリとコンパイル状態の表示
    pub(in crate::app) fn shader_status(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("シェーダー(開発)");
        ui.label(format!("{} を監視中", shader_dir().display()));
        match &self.shader_error {
            None => {
                ui.colored_label(egui::Color32::from_rgb(64, 160, 64), "コンパイル OK");
            }
            Some(_) => {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "コンパイルエラー");
            }
        }
    }
}
