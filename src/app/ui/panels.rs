//! ストローク記録再生(H5)・シェーダー状態(H1)・操作結果表示のパネル。
//! 設定プリセット(H3)は上部ファイルメニュー([file_menu])のモーダルへ移した。

use crate::app::PaintApp;
use crate::gpu::hot_reload::shader_dir;
use crate::replay::{self, Recorder, Recording};
use crate::video::{self, VideoJob, VideoSettings};
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

    /// ストローク記録・再生(H5): 記録操作・保存・一覧再生。名前保存+一覧は NamedStore
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
        // 保存済み一覧: 行ごとに再生と動画書き出し
        let mut video_name: Option<String> = None;
        let exporting = self.replay_ui.video.is_some();
        for name in &self.replay_ui.store.list {
            ui.horizontal(|ui| {
                if ui.button("▶ 再生").clicked() {
                    match replay::load(name) {
                        Ok(stored) => replay_now = Some((stored.recording, stored.palette)),
                        Err(e) => self.status_msg = Some(e),
                    }
                }
                if ui
                    .add_enabled(!exporting, egui::Button::new("🎬 動画"))
                    .on_hover_text("記録を裏で再生して MP4 に書き出す(ffmpeg を使う)。いまのキャンバスは変えない")
                    .clicked()
                {
                    video_name = Some(name.clone());
                }
                ui.label(name);
            });
        }
        if let Some((recording, palette)) = replay_now {
            self.start_replay(recording, palette);
        }
        if let Some(name) = video_name {
            self.start_video_export(name);
        }
        if let Some(job) = &self.replay_ui.video {
            ui.horizontal(|ui| {
                let done = job.done().min(job.total);
                ui.add(
                    egui::ProgressBar::new(done as f32 / job.total.max(1) as f32)
                        .desired_width(160.0)
                        .text(format!("{} {done}/{}", job.name, job.total)),
                );
                if ui.button("取り消し").clicked() {
                    job.cancel();
                }
            });
        }
        // status_msg の表示は status_bar(常時可視・スクロール外)へ移した(F3)
    }

    /// 保存済みストロークを動画に書き出し始める。キャンバスサイズ・パラメータは今の値、
    /// パレットは記録時のもの(画面の再生と同じ条件)。書き出しは別スレッドの別キャンバスで行う
    fn start_video_export(&mut self, name: String) {
        let stored = match replay::load(&name) {
            Ok(s) => s,
            Err(e) => {
                self.status_msg = Some(e);
                return;
            }
        };
        let settings = VideoSettings {
            size: self.canvas_size,
            params: self.params,
            steps_per_frame: self.steps_per_frame,
            fps: video::VIDEO_FPS,
            tail_frames: video::TAIL_FRAMES,
        };
        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let path = video::videos_dir().join(format!("{stamp}_{name}.mp4"));
        self.replay_ui.video = Some(VideoJob::spawn(
            self.render_state.device.clone(),
            self.render_state.queue.clone(),
            name,
            stored.recording,
            stored.palette,
            settings,
            path,
        ));
    }

    /// 動画書き出しが終わっていれば結果を status_msg に出して片付ける。開発モードでパネルを
    /// 閉じていても拾えるよう、毎フレーム ui() から呼ぶ
    pub(in crate::app) fn poll_video_export(&mut self) {
        let Some(job) = &mut self.replay_ui.video else {
            return;
        };
        if let Some(result) = job.poll() {
            self.status_msg = Some(match result {
                Ok(path) => format!("動画を保存: {}", path.display()),
                Err(e) => e,
            });
            self.replay_ui.video = None;
        }
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
        if cfg!(feature = "embed-assets") {
            ui.label("埋め込みシェーダー使用中(ホットリロード無効)");
        } else {
            ui.label(format!("{} を監視中", shader_dir().display()));
        }
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
