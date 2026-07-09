//! ストローク記録再生(H5)・シェーダー状態(H1)・操作結果表示のパネル。app/mod.rs から分割(R4)。
//! 設定プリセット(H3)は上部ファイルメニュー([file_menu])のモーダルへ移した。

use crate::app::PaintApp;
use crate::gpu::hot_reload::shader_dir;
use crate::replay::{self, Recorder, Recording};
use eframe::egui;

impl PaintApp {
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
        // M5d: 保存には記録時のパレット(現行パレット)を添える。試し再生も現行パレット
        let mut replay_now: Option<(Recording, pigment::Palette)> = None;
        if let Some(recording) = self.replay_ui.pending_recording.clone() {
            let palette = self.palette.clone();
            if let Some(status) = self.replay_ui.store.save_controls(
                ui,
                "ストローク名",
                |name| replay::save(name, &recording, &palette),
                replay::list,
            ) {
                self.status_msg = Some(status);
            }
            if ui.button("試し再生").clicked() {
                replay_now = Some((recording, self.palette.clone()));
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
        // 右パネル最上段(レイヤーの上)に置くので、先頭の区切り線は付けない
        ui.add_space(4.0);
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
