//! egui の画面構成: パラメータパネル(H2)、プリセット保存/読込(H3)、
//! デバッグ表示切替(H4)、ストローク記録・再生(H5)、シミュレーション制御・
//! PNG スナップショット(H6)、キャンバス、シェーダーエラーのオーバーレイ(H1)。

use crate::brush::StrokeState;
use crate::gpu::hot_reload::{ShaderWatcher, shader_dir};
use crate::gpu::{CanvasCallback, GpuCanvas};
use crate::input::{MouseSource, PointerEvent, PointerPhase, PointerSource, TabletSource};
use crate::pigment::PIGMENTS;
use crate::preset;
use crate::replay::{self, Player, Recorder, Recording};
use crate::sim::{CANVAS_SIZE, SimParams, Splat};
use eframe::egui;
use eframe::egui_wgpu;
use std::path::{Path, PathBuf};

/// デバッグ表示モード(H4)。値は SimParams::display_mode / display.wgsl の分岐と対応。
const DISPLAY_MODES: [(u32, &str); 7] = [
    (0, "通常(顔料を表示)"),
    (1, "水量ヒートマップ"),
    (2, "速度場(色相=方向)"),
    (3, "湿りオーバーレイ(濡れ=青)"),
    (4, "浮遊顔料ヒートマップ"),
    (5, "沈着顔料ヒートマップ"),
    (6, "紙ハイト(白=山)"),
];

/// egui のデフォルトフォントは日本語グリフを含まないため、
/// Windows のシステムフォントをフォールバックとして追加する(バンドル不要)。
fn install_japanese_font(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];
    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "japanese".to_owned(),
            egui::FontData::from_owned(bytes).into(),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("japanese".to_owned());
        }
        ctx.set_fonts(fonts);
        return;
    }
    log::warn!("日本語フォントが見つかりませんでした。UI の日本語は豆腐表示になります");
}

pub struct PaintApp {
    render_state: egui_wgpu::RenderState,
    params: SimParams,
    stroke: StrokeState,
    /// M1.5: ペン入力(octotablet、筆圧)。接続失敗時はマウスのみで動く
    tablet: TabletSource,
    mouse: MouseSource,
    /// ストローク中(Down〜Up の間)。キャンバス外で Down したときは立たない
    painting: bool,
    watcher: ShaderWatcher,
    /// 直近のシェーダービルドエラー(H1: 落とさずオーバーレイ表示)
    shader_error: Option<String>,
    /// H6: 一時停止中はシミュレーションステップを回さない(splat は反映される)
    paused: bool,
    /// H6: 一時停止中の「1ステップ」ボタンが押された(次フレームで消費)
    step_once: bool,
    /// H6: 速度倍率(1フレームあたりのシミュレーションステップ数)
    steps_per_frame: u32,
    /// H3: プリセットの保存名入力と一覧(一覧はキャッシュ。保存時と ↻ で更新)
    preset_name: String,
    preset_list: Vec<String>,
    /// H5: ストロークの保存名入力と一覧
    stroke_name: String,
    stroke_list: Vec<String>,
    /// H5: 記録中の状態(Some の間はポインタ入力を記録)
    recorder: Option<Recorder>,
    /// H5: 記録停止後、保存/試し再生できる直近の記録
    pending_recording: Option<Recording>,
    /// H5: 再生中の状態(Some の間は記録済み入力を毎フレーム流し込む)
    player: Option<Player>,
    /// H3/H5/H6 の操作結果の表示(保存先パスやエラー)
    status_msg: Option<String>,
}

impl PaintApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_japanese_font(&cc.egui_ctx);

        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("wgpu レンダラーが必要です(NativeOptions.renderer を確認)");

        let dir = shader_dir();
        let mut canvas = GpuCanvas::new(
            &render_state.device,
            &render_state.queue,
            render_state.target_format,
            dir.clone(),
        );
        let shader_error = canvas.rebuild_pipelines(&render_state.device).err();
        if let Some(e) = &shader_error {
            log::error!("WGSL の初回ビルドに失敗: {e}");
        }
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(canvas);

        Self {
            render_state,
            params: SimParams::default(),
            stroke: StrokeState::default(),
            tablet: TabletSource::new(cc),
            mouse: MouseSource,
            painting: false,
            watcher: ShaderWatcher::new(&dir),
            shader_error,
            paused: false,
            step_once: false,
            steps_per_frame: 1,
            preset_name: String::new(),
            preset_list: preset::list(),
            stroke_name: String::new(),
            stroke_list: replay::list(),
            recorder: None,
            pending_recording: None,
            player: None,
            status_msg: None,
        }
    }

    fn rebuild_shaders(&mut self) {
        let mut renderer = self.render_state.renderer.write();
        if let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() {
            self.shader_error = canvas
                .rebuild_pipelines(&self.render_state.device)
                .err();
        }
    }

    fn clear_canvas(&self) {
        let renderer = self.render_state.renderer.read();
        if let Some(canvas) = renderer.callback_resources.get::<GpuCanvas>() {
            canvas.clear(&self.render_state.queue);
        }
    }

    /// H5: キャンバスをリセットして記録済みストロークの再生を始める
    /// (同一入力での A/B 比較のため、必ず白紙から)
    fn start_replay(&mut self, recording: Recording) {
        self.clear_canvas();
        self.stroke.end();
        self.painting = false;
        self.player = Some(Player::new(recording));
    }

    /// 正規化ポインタイベント(input.rs)をストローク・記録(H5)・splat 列へ反映する。
    /// マウスとペンの共通経路。座標はウィンドウ論理ピクセル → キャンバステクセルに変換
    fn apply_pointer_events(
        &mut self,
        events: &[PointerEvent],
        rect: egui::Rect,
        splats: &mut Vec<Splat>,
    ) {
        let scale = CANVAS_SIZE as f32 / rect.width();
        for ev in events {
            match ev.phase {
                PointerPhase::Down => {
                    // キャンバス外での筆下ろしは無視(UI パネル上のペン操作など)
                    if !rect.contains(ev.pos) {
                        continue;
                    }
                    self.painting = true;
                    self.stroke.begin();
                    // H5: 記録はストローク単位。そのとき選ばれていた顔料スロットも残す
                    if let Some(recorder) = &mut self.recorder {
                        recorder.begin_stroke(self.params.brush_channel);
                    }
                }
                PointerPhase::Move => {}
                PointerPhase::Up => {
                    if self.painting {
                        self.painting = false;
                        self.stroke.end();
                        if let Some(recorder) = &mut self.recorder {
                            recorder.end_stroke();
                        }
                    }
                    continue;
                }
            }
            if !self.painting {
                continue;
            }
            let px = (ev.pos - rect.min) * scale;
            // サンプル間隔は筆圧を反映した実効半径から(細い筆入れでも隙間を作らない)
            let spacing = (self.params.radius_at(ev.pressure) * 0.25).max(1.0);
            self.stroke
                .add_motion([px.x, px.y], ev.pressure, spacing, splats);
            // H5: 補間前の生ポインタ位置+筆圧を記録する(再生時に補間し直すため
            // ブラシ半径や筆圧マッピングを変えても同じストロークを引ける)
            if let Some(recorder) = &mut self.recorder {
                recorder.add_point([px.x, px.y], ev.pressure);
            }
        }
    }

    /// H6: 現在のキャンバス表示を snapshots/ に PNG 保存する。
    /// ファイル名はタイムスタンプ+プリセット名(入力欄の値。「どの設定の絵か」を残す)
    fn save_snapshot(&mut self) {
        let result = (|| -> Result<PathBuf, String> {
            let data = {
                let renderer = self.render_state.renderer.read();
                let canvas = renderer
                    .callback_resources
                    .get::<GpuCanvas>()
                    .ok_or("キャンバスが初期化されていません")?;
                canvas.snapshot(&self.render_state.device, &self.render_state.queue)?
            };
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots");
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
            let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let preset = self.preset_name.trim();
            let name = if preset.is_empty() {
                format!("{stamp}.png")
            } else {
                format!("{stamp}_{preset}.png")
            };
            let path = dir.join(name);
            image::save_buffer(
                &path,
                &data,
                CANVAS_SIZE,
                CANVAS_SIZE,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| format!("PNG の書き出しに失敗: {e}"))?;
            Ok(path)
        })();
        self.status_msg = Some(match result {
            Ok(path) => format!("保存: {}", path.display()),
            Err(e) => e,
        });
    }

    fn tool_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("水ブラシ");
        // 顔料セレクタ(M1c): ブラシが注入する顔料スロットを選ぶ
        ui.horizontal(|ui| {
            for (i, pigment) in PIGMENTS.iter().enumerate() {
                let selected = self.params.brush_channel == i as u32;
                let color =
                    egui::Color32::from_rgb(pigment.rgb[0], pigment.rgb[1], pigment.rgb[2]);
                let mut button = egui::Button::new("").fill(color).min_size(egui::vec2(28.0, 28.0));
                if selected {
                    button = button.stroke((2.0, ui.visuals().strong_text_color()));
                }
                if ui.add(button).on_hover_text(pigment.name).clicked() {
                    self.params.brush_channel = i as u32;
                }
            }
        });
        ui.label(PIGMENTS[self.params.brush_channel.min(3) as usize].name);
        ui.add(
            egui::Slider::new(&mut self.params.brush_radius, 1.0..=64.0)
                .text("半径")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.brush_water, 0.0..=2.0).text("水量"));
        ui.add(egui::Slider::new(&mut self.params.brush_velocity, 0.0..=2.0).text("初速"));
        ui.add(egui::Slider::new(&mut self.params.brush_pigment, 0.0..=2.0).text("顔料量(0=水筆)"));

        ui.separator();
        ui.heading("筆圧 (M1.5)");
        match self.tablet.error() {
            None => match self.tablet.last_pressure() {
                Some(p) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(64, 160, 64),
                        format!("ペン検知中(筆圧 {p:.2})"),
                    );
                }
                None => {
                    ui.label("ペン接続 OK(検知範囲外)");
                }
            },
            Some(e) => {
                ui.label("ペン未接続(マウスのみ)").on_hover_text(e);
            }
        }
        ui.add(egui::Slider::new(&mut self.params.pressure_radius, 0.0..=1.0).text("筆圧→半径の効き"));
        ui.add(egui::Slider::new(&mut self.params.pressure_water, 0.0..=1.0).text("筆圧→水量の効き"));
        ui.add(egui::Slider::new(&mut self.params.pressure_pigment, 0.0..=1.0).text("筆圧→顔料量の効き"));
        ui.add(
            egui::Slider::new(&mut self.params.pressure_gamma, 0.25..=4.0)
                .logarithmic(true)
                .text("応答カーブ γ(>1で軽いタッチが細く)"),
        );

        ui.separator();
        ui.heading("水シミュレーション (M1a)");
        ui.add(egui::Slider::new(&mut self.params.dt, 0.05..=1.0).text("時間刻み dt"));
        ui.add(egui::Slider::new(&mut self.params.accel, 0.0..=4.0).text("移流強度(勾配→加速)"));
        ui.add(egui::Slider::new(&mut self.params.damping, 0.0..=0.5).text("速度減衰"));
        ui.add(egui::Slider::new(&mut self.params.xi, 0.0..=0.5).text("発散緩和 ξ"));
        ui.add(egui::Slider::new(&mut self.params.relax_iters, 1..=50).text("緩和反復回数"));
        ui.add(egui::Slider::new(&mut self.params.vel_max, 0.1..=2.0).text("速度上限 (CFL)"));
        ui.add(egui::Slider::new(&mut self.params.wet_expand, 0.0..=0.5).text("にじみ拡張(0=固定マスク)"));

        ui.separator();
        ui.heading("顔料 (M1b)");
        ui.add(egui::Slider::new(&mut self.params.pigment_diffuse, 0.0..=1.0).text("拡散率(にじみの速さ)"));
        ui.add(egui::Slider::new(&mut self.params.diffuse_iters, 0..=32).text("拡散反復回数(速いにじみはこちらで)"));
        ui.add(egui::Slider::new(&mut self.params.deposit_rate, 0.0..=0.5).text("吸着率(沈着の速さ)"));
        ui.add(egui::Slider::new(&mut self.params.lift_rate, 0.0..=0.5).text("脱着率(再浮遊の速さ)"));
        ui.add(
            egui::Slider::new(&mut self.params.evap_rate, 0.0..=0.05)
                .logarithmic(true)
                .text("蒸発率"),
        );
        ui.add(
            egui::Slider::new(&mut self.params.pigment_density, 0.5..=10.0)
                .logarithmic(true)
                .text("発色の濃さ(濃度→被覆率)"),
        );

        ui.separator();
        ui.heading("紙・エッジ (M1d)");
        ui.add(egui::Slider::new(&mut self.params.paper_amp, 0.0..=1.0).text("紙ハイト振幅(谷へ流す)"));
        ui.add(egui::Slider::new(&mut self.params.paper_gran, 0.0..=1.0).text("粒状化(凹部に沈着)"));
        ui.add(egui::Slider::new(&mut self.params.paper_wet, 0.0..=1.0).text("にじみ縁の紙目変調"));
        ui.add(
            egui::Slider::new(&mut self.params.edge_eta, 0.0..=0.2)
                .text("エッジダークニング η(0=無効)"),
        );
        ui.add(egui::Slider::new(&mut self.params.edge_radius, 1..=8).text("縁バンド幅(ぼかし半径)"));

        ui.separator();
        ui.heading("表示 (H4)");
        let mode_label = |mode: u32| {
            DISPLAY_MODES
                .iter()
                .find(|(v, _)| *v == mode)
                .map_or("?", |(_, label)| *label)
        };
        egui::ComboBox::from_label("表示モード")
            .selected_text(mode_label(self.params.display_mode))
            .show_ui(ui, |ui| {
                for (value, label) in DISPLAY_MODES {
                    ui.selectable_value(&mut self.params.display_mode, value, label);
                }
            });
        ui.add(
            egui::Slider::new(&mut self.params.display_gain, 0.1..=10.0)
                .logarithmic(true)
                .text("表示ゲイン"),
        );

        ui.separator();
        ui.heading("制御 (H6)");
        ui.horizontal(|ui| {
            let pause_label = if self.paused { "▶ 再開" } else { "⏸ 一時停止" };
            if ui.button(pause_label).clicked() {
                self.paused = !self.paused;
            }
            if ui
                .add_enabled(self.paused, egui::Button::new("1ステップ"))
                .clicked()
            {
                self.step_once = true;
            }
        });
        ui.add(
            egui::Slider::new(&mut self.steps_per_frame, 1..=8)
                .text("速度倍率")
                .suffix(" ステップ/フレーム"),
        );
        ui.horizontal(|ui| {
            if ui.button("キャンバスをリセット").clicked() {
                self.clear_canvas();
            }
            if ui.button("PNG スナップショット").clicked() {
                self.save_snapshot();
            }
        });

        ui.separator();
        ui.heading("プリセット (H3)");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.preset_name)
                    .hint_text("プリセット名")
                    .desired_width(140.0),
            );
            let name = self.preset_name.trim().to_owned();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("保存"))
                .clicked()
            {
                self.status_msg = Some(match preset::save(&name, &self.params) {
                    Ok(path) => {
                        self.preset_list = preset::list();
                        format!("保存: {}", path.display())
                    }
                    Err(e) => e,
                });
            }
            if ui.button("↻").on_hover_text("一覧を再読込").clicked() {
                self.preset_list = preset::list();
            }
        });
        let mut load_preset: Option<String> = None;
        for name in &self.preset_list {
            ui.horizontal(|ui| {
                if ui.button("読込").clicked() {
                    load_preset = Some(name.clone());
                }
                ui.label(name);
            });
        }
        if let Some(name) = load_preset {
            match preset::load(&name) {
                Ok(params) => {
                    self.params = params;
                    self.preset_name = name;
                }
                Err(e) => self.status_msg = Some(e),
            }
        }

        ui.separator();
        ui.heading("ストローク記録・再生 (H5)");
        ui.horizontal(|ui| {
            match &self.recorder {
                None => {
                    if ui.button("⏺ 記録開始").clicked() {
                        self.recorder = Some(Recorder::new());
                        self.pending_recording = None;
                    }
                }
                Some(_) => {
                    if ui.button("⏹ 記録停止").clicked() {
                        let recording = self.recorder.take().unwrap().finish();
                        if recording.is_empty() {
                            self.status_msg = Some("ストロークが記録されていません".to_owned());
                        } else {
                            self.pending_recording = Some(recording);
                        }
                    }
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "記録中…");
                }
            }
            if self.player.is_some() {
                if ui.button("■ 再生停止").clicked() {
                    self.player = None;
                }
                ui.colored_label(egui::Color32::from_rgb(64, 160, 64), "再生中…");
            }
        });
        // 記録直後: 名前を付けて保存 or そのまま試し再生
        let mut replay_now: Option<Recording> = None;
        if let Some(recording) = &self.pending_recording {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.stroke_name)
                        .hint_text("ストローク名")
                        .desired_width(140.0),
                );
                let name = self.stroke_name.trim().to_owned();
                if ui
                    .add_enabled(!name.is_empty(), egui::Button::new("保存"))
                    .clicked()
                {
                    self.status_msg = Some(match replay::save(&name, recording) {
                        Ok(path) => {
                            self.stroke_list = replay::list();
                            format!("保存: {}", path.display())
                        }
                        Err(e) => e,
                    });
                }
                if ui.button("試し再生").clicked() {
                    replay_now = Some(recording.clone());
                }
            });
        }
        let mut load_stroke: Option<String> = None;
        for name in &self.stroke_list {
            ui.horizontal(|ui| {
                if ui.button("▶ 再生").clicked() {
                    load_stroke = Some(name.clone());
                }
                ui.label(name);
            });
        }
        if let Some(name) = load_stroke {
            match replay::load(&name) {
                Ok(recording) => replay_now = Some(recording),
                Err(e) => self.status_msg = Some(e),
            }
        }
        if let Some(recording) = replay_now {
            self.start_replay(recording);
        }

        if let Some(msg) = &self.status_msg {
            ui.separator();
            ui.label(msg.clone());
        }

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

    fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        // 正方形キャンバスを利用可能領域の中央に置く
        let available = ui.available_size();
        let side = available.min_elem().max(64.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(side, side),
            egui::Sense::drag(),
        );

        // M1.5: ペン(octotablet)は毎フレーム pump する。ペンが検知範囲内にいる間は
        // ペンを採用しマウスは無視する(Windows Ink のペンは OS がカーソルも動かすため、
        // 両方を処理すると二重ストロークになる)
        let tablet_events = self.tablet.poll(&response);
        let events = if self.tablet.is_active() || !tablet_events.is_empty() {
            tablet_events
        } else {
            self.mouse.poll(&response)
        };

        let mut splats: Vec<Splat> = Vec::new();
        self.apply_pointer_events(&events, rect, &mut splats);

        // H5: 記録はフレーム基準(ストローク間の待ちも再現される)
        if let Some(recorder) = &mut self.recorder {
            recorder.tick();
        }

        // H5: 再生中は記録済みポインタ入力を同じテンポで流し込む(手描きと合流可)
        if let Some(player) = &mut self.player
            && !player.advance(&mut self.params, &mut splats)
        {
            self.player = None;
        }

        // H6: 一時停止中は 0 ステップ(1ステップボタンが押されていれば 1)
        let sim_steps = if self.paused {
            if std::mem::take(&mut self.step_once) { 1 } else { 0 }
        } else {
            self.steps_per_frame
        };

        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            CanvasCallback {
                params: self.params,
                splats,
                sim_steps,
            },
        ));
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            (1.0, ui.visuals().weak_text_color()),
            egui::StrokeKind::Outside,
        );
    }

    fn error_overlay(&self, ui: &mut egui::Ui) {
        let Some(error) = &self.shader_error else {
            return;
        };
        egui::Panel::bottom("shader_error")
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(60, 16, 16))
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 140, 140),
                    "WGSL コンパイルエラー(直前の正常なシェーダーで継続中。保存し直すと再試行):",
                );
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(error).monospace().color(egui::Color32::WHITE),
                        )
                        .wrap(),
                    );
                });
            });
    }
}

impl eframe::App for PaintApp {
    /// ウィンドウ破棄前にタブレット接続を切る(TabletSource::new の安全条件)
    fn on_exit(&mut self) {
        self.tablet.disconnect();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // H1: .wgsl が保存されたら再ビルド(失敗しても落とさない)
        if self.watcher.take_dirty() {
            self.rebuild_shaders();
        }

        egui::Panel::left("tools")
            .default_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.tool_panel(ui));
            });

        self.error_overlay(ui);

        egui::CentralPanel::default().show(ui, |ui| self.canvas_ui(ui));

        // 常時シミュレーションが走るため連続再描画
        ui.ctx().request_repaint();
    }
}
