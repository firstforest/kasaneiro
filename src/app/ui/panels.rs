//! プリセット(H3)・ストローク記録再生(H5)・シェーダー状態(H1)のパネル。
//! app/mod.rs から分割(R4)。

use crate::app::PaintApp;
use crate::gpu::hot_reload::shader_dir;
use crate::preset;
use crate::replay::{self, Recorder, Recording};
use eframe::egui;

impl PaintApp {
    /// プリセット(H3): 名前保存+一覧読込。共通 UI は NamedStore に集約(R4)
    pub(in crate::app) fn preset_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("プリセット (H3)");
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
                    self.replay_ui.player = None;
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

        if let Some(msg) = &self.status_msg {
            ui.separator();
            ui.label(msg.clone());
        }
    }

    /// ビュー(M6): 拡大率の表示・全体表示に戻す・操作ヒント。
    /// 拡大/パンの実操作はキャンバス上のホイール・中ボタンドラッグで行う(canvas.rs)
    pub(in crate::app) fn view_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("ビュー (M6)");
        ui.horizontal(|ui| {
            ui.label(format!("拡大 {:.0}%", self.view_zoom * 100.0));
            if ui
                .add_enabled(self.view_zoom > 1.0, egui::Button::new("全体表示に戻す"))
                .clicked()
            {
                self.reset_view();
            }
        });
        ui.label(
            egui::RichText::new("ホイール=カーソル中心に拡大 / 中ボタンドラッグ=パン")
                .weak()
                .small(),
        );
    }

    /// シェーダー(H1): 監視ディレクトリとコンパイル状態の表示
    pub(in crate::app) fn shader_status(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("シェーダー (H1)");
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
