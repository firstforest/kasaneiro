//! egui の画面構成: パラメータパネル(H2)、プリセット保存/読込(H3)、
//! デバッグ表示切替(H4)、ストローク記録・再生(H5)、シミュレーション制御・
//! PNG スナップショット(H6)、キャンバス、シェーダーエラーのオーバーレイ(H1)。

mod ui;

use crate::gpu::hot_reload::{ShaderWatcher, shader_dir};
use crate::gpu::{CanvasCallback, GpuCanvas, MAX_LAYERS};
use crate::input::{MouseSource, PenSource, PointerEvent, PointerPhase, PointerSource};
use crate::preset;
use crate::replay::{self, Player, Recorder, Recording};
use paint_core::brush::StrokeState;
use paint_core::sim::{CANVAS_SIZE, SimParams, Splat};
use paint_core::tool::{Tool, ToolInfo, WetTool};
use pigment::PIGMENTS;
use ui::{NamedStore, PresetUi, ReplayUi};
use eframe::egui;
use eframe::egui_wgpu;
use std::path::{Path, PathBuf};

/// レイヤー合成方式(M3)。値は SimParams::compose_mode / display.wgsl の分岐と対応。
const COMPOSE_MODES: [(u32, &str, &str); 2] = [
    (0, "multiply", "M2 の乗算合成(重ねるほど暗く。散乱を無視した安価な近似)"),
    (
        1,
        "KM(R/T)",
        "Kubelka-Munk の光学合成(M3)。各層を白地/黒地に置いた発色から反射率・透過率を導き、下から光学混色する。薄い層ほど下が透ける「内側から光る」グレーズ",
    ),
];

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

/// UI フォントを M PLUS 1p(Google Fonts / OFL、`assets/fonts/`)に差し替える。
/// egui のデフォルトフォントは日本語グリフを含まないため、実行ファイルに埋め込んだ
/// M PLUS 1p を最優先に据える。M PLUS 1p が持たないグリフ用に、あれば Windows の
/// システム日本語フォントをフォールバックとして後ろに足す。
fn install_japanese_font(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];
    let mut fonts = egui::FontDefinitions::default();
    // include_bytes! のパスはこのソースファイル基準(src/app/mod.rs → リポジトリルート)。
    fonts.font_data.insert(
        "mplus1p".to_owned(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/MPLUS1p-Regular.ttf"))
            .into(),
    );

    // 欠落グリフ用フォールバック(見つからなければスキップ。M PLUS 1p だけで動く)。
    let system_fallback = CANDIDATES.iter().find_map(|path| {
        let bytes = std::fs::read(path).ok()?;
        fonts.font_data.insert(
            "japanese_fallback".to_owned(),
            egui::FontData::from_owned(bytes).into(),
        );
        Some(())
    });

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        // M PLUS 1p を最優先(先頭)。既存のデフォルトフォントはその後ろに残す。
        list.insert(0, "mplus1p".to_owned());
        if system_fallback.is_some() {
            list.push("japanese_fallback".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

pub struct PaintApp {
    render_state: egui_wgpu::RenderState,
    params: SimParams,
    /// 選択中ツール(R2)。トップレベルの分岐が描画経路の分岐。wet ツールは
    /// `WetTool::gpu_id()` を `params.tool` へ同期して GPU の splat 分岐に渡す。
    /// ラスタツール(M4.5)は流体経路に流れない
    tool: Tool,
    stroke: StrokeState,
    /// M1.5: ペン入力(egui Touch 経由、筆圧付き)。接地中はマウスより優先される
    pen: PenSource,
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
    /// H3: プリセットの UI 状態(名前入力+一覧。R4 で集約)
    preset_ui: PresetUi,
    /// H5: ストローク記録・再生の UI 状態(名前+一覧+recorder/pending/player。R4 で集約)
    replay_ui: ReplayUi,
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
            tool: Tool::Wet(WetTool::Paint),
            stroke: StrokeState::default(),
            pen: PenSource::default(),
            mouse: MouseSource,
            painting: false,
            watcher: ShaderWatcher::new(&dir),
            shader_error,
            paused: false,
            step_once: false,
            steps_per_frame: 1,
            preset_ui: PresetUi {
                store: NamedStore::new(preset::list()),
            },
            replay_ui: ReplayUi {
                store: NamedStore::new(replay::list()),
                recorder: None,
                pending_recording: None,
                player: None,
            },
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
        let mut renderer = self.render_state.renderer.write();
        if let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() {
            canvas.clear(&self.render_state.queue);
        }
    }

    /// M2 の手動アクション(乾かす / Fast Dry / 再湿潤)を GpuCanvas 上で実行する共通経路。
    /// 失敗(シェーダー未ビルド・レイヤー上限)はステータス表示に流す
    fn run_canvas_action(
        &mut self,
        action: impl FnOnce(
            &mut GpuCanvas,
            &egui_wgpu::wgpu::Device,
            &egui_wgpu::wgpu::Queue,
        ) -> Result<(), String>,
    ) {
        let result = {
            let mut renderer = self.render_state.renderer.write();
            match renderer.callback_resources.get_mut::<GpuCanvas>() {
                Some(canvas) => action(
                    canvas,
                    &self.render_state.device,
                    &self.render_state.queue,
                ),
                None => Err("キャンバスが初期化されていません".to_owned()),
            }
        };
        if let Err(e) = result {
            self.status_msg = Some(e);
        }
    }

    /// M2: 乾燥操作は「にじみを止めたい瞬間」に間に合う必要がある(Fresco の UX 教訓)ため、
    /// スクロール領域の外=左パネル最上部に常時表示する
    fn dry_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .button(egui::RichText::new("乾かす").strong())
                .on_hover_text("定着パスを走らせて乾燥レイヤーへ焼き込み、湿レイヤーを空にする(M2)")
                .clicked()
            {
                self.run_canvas_action(|c, d, q| c.bake_dry(d, q));
            }
            if ui
                .button("水だけ除去")
                .on_hover_text("Fast Dry: 水と流れを止め、浮遊顔料をその場で沈着(焼き込みはしない)")
                .clicked()
            {
                self.run_canvas_action(|c, d, q| c.fast_dry(d, q));
            }
            if ui
                .button("全面を湿らす")
                .on_hover_text("Wet the Layer: キャンバス全面を濡らす(水量は「再湿潤の水量」スライダー)")
                .clicked()
            {
                self.run_canvas_action(|c, d, q| c.rewet(d, q));
            }
        });
        ui.add_space(4.0);
    }

    /// M2: レイヤーパネル(可視性・並べ替え)。乾燥レイヤーは焼き込み後は編集不可で、
    /// multiply 合成では順序は見た目に効かない(KM 合成 M3 で効く)が配管は通しておく
    fn layer_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("レイヤー (M2)");
        let mut renderer = self.render_state.renderer.write();
        let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() else {
            return;
        };
        ui.label(format!(
            "湿レイヤー(描画先)+ 乾燥 {}/{} 枚",
            canvas.layers.len(),
            MAX_LAYERS
        ));
        let count = canvas.layers.len();
        let mut changed = false;
        let mut swap: Option<(usize, usize)> = None;
        // 上から表示(Vec の末尾=最後に乾かしたもの=最上層)
        for k in (0..count).rev() {
            ui.horizontal(|ui| {
                let layer = &mut canvas.layers[k];
                if ui
                    .checkbox(&mut layer.visible, format!("乾燥レイヤー {}", layer.slot + 1))
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .add_enabled(k + 1 < count, egui::Button::new("⬆"))
                    .clicked()
                {
                    swap = Some((k, k + 1));
                }
                if ui.add_enabled(k > 0, egui::Button::new("⬇")).clicked() {
                    swap = Some((k, k - 1));
                }
            });
        }
        if let Some((a, b)) = swap {
            canvas.layers.swap(a, b);
            changed = true;
        }
        if changed {
            canvas.sync_layers(&self.render_state.queue);
        }
    }

    /// H5: キャンバスをリセットして記録済みストロークの再生を始める
    /// (同一入力での A/B 比較のため、必ず白紙から)
    fn start_replay(&mut self, recording: Recording) {
        self.clear_canvas();
        self.stroke.end();
        self.painting = false;
        self.replay_ui.player = Some(Player::new(recording));
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
                    // H5: 記録はストローク単位。そのとき選ばれていた顔料スロットとツールも残す
                    if let Some(recorder) = &mut self.replay_ui.recorder {
                        recorder.begin_stroke(self.params.brush_channel, self.params.tool);
                    }
                }
                PointerPhase::Move => {}
                PointerPhase::Up => {
                    if self.painting {
                        self.painting = false;
                        self.stroke.end();
                        if let Some(recorder) = &mut self.replay_ui.recorder {
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
            if let Some(recorder) = &mut self.replay_ui.recorder {
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
            let preset = self.preset_ui.store.name.trim();
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

    /// 左パネルのスクロール内容。セクションごとのメソッドへ振り分けるだけ(R4)。
    /// M4.5/M5 でセクションが増えてもこのディスパッチャに1行足すだけで済む
    fn tool_panel(&mut self, ui: &mut egui::Ui) {
        self.brush_panel(ui);
        self.layers_panel(ui);
        self.tuning_panel(ui);
        self.preset_panel(ui);
        self.replay_panel(ui);
        self.shader_status(ui);
    }

    /// 水ブラシ(M1〜M4): ツール選択・顔料スロット・ブラシスライダー
    fn brush_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("水ブラシ");
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
                }
            }
        });
        if let Some(wt) = self.tool.wet() {
            self.params.tool = wt.gpu_id();
        }
        // 顔料セレクタ(M1c): ブラシが注入する顔料スロットを選ぶ。ホバーで顔料個性(M3)を表示
        ui.horizontal(|ui| {
            for (i, pigment) in PIGMENTS.iter().enumerate() {
                let selected = self.params.brush_channel == i as u32;
                let color =
                    egui::Color32::from_rgb(pigment.rgb[0], pigment.rgb[1], pigment.rgb[2]);
                let mut button = egui::Button::new("").fill(color).min_size(egui::vec2(28.0, 28.0));
                if selected {
                    button = button.stroke((2.0, ui.visuals().strong_text_color()));
                }
                let hover = format!(
                    "{}\n密度 ρ={:.2} / ステイニング ω={:.2} / 粒状感 γ={:.2}",
                    pigment.name, pigment.density, pigment.staining, pigment.granulation
                );
                if ui.add(button).on_hover_text(hover).clicked() {
                    self.params.brush_channel = i as u32;
                }
            }
        });
        let pg = &PIGMENTS[self.params.brush_channel.min(3) as usize];
        ui.label(format!(
            "{}(ω={:.2} γ={:.2})",
            pg.name, pg.staining, pg.granulation
        ));
        ui.add(
            egui::Slider::new(&mut self.params.brush_radius, 1.0..=64.0)
                .text("半径")
                .suffix(" px"),
        );
        ui.add(egui::Slider::new(&mut self.params.brush_water, 0.0..=2.0).text("水量"));
        ui.add(egui::Slider::new(&mut self.params.brush_velocity, 0.0..=2.0).text("初速"));
        ui.add(egui::Slider::new(&mut self.params.brush_pigment, 0.0..=1.0).text("顔料量(0=水筆)"));
        ui.add(
            egui::Slider::new(&mut self.params.lift_strength, 0.0..=1.0)
                .text("リフト強度(削りツール)"),
        );
        ui.add(
            egui::Slider::new(&mut self.params.water_lift, 0.0..=1.0)
                .text("水筆の均し強度(均一さ)"),
        );
        ui.add(
            egui::Slider::new(&mut self.params.smear_rate, 0.0..=1.0)
                .text("ならし強度(濃い山を伸ばす)"),
        );
    }

    /// レイヤー(M2): 乾燥レイヤーの可視性・並べ替え + 合成方式(M3)
    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        self.layer_panel(ui);
        // レイヤー合成方式(M3): multiply(M2)⇔ KM の R/T 合成を切替。H5 再生で A/B 比較
        ui.horizontal(|ui| {
            ui.label("合成:");
            for (value, label, hover) in COMPOSE_MODES {
                ui.selectable_value(&mut self.params.compose_mode, value, label)
                    .on_hover_text(hover);
            }
        });
    }

    /// 乾燥・筆圧・味付けスライダー・診断表示・シミュ制御(H6)をまとめた調整セクション
    fn tuning_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("乾燥 (M2)");
        ui.add(
            egui::Slider::new(&mut self.params.dry_shift, 0.0..=1.5)
                .text("乾燥シフト(<1で乾くと薄く)"),
        );
        ui.add(
            egui::Slider::new(&mut self.params.rewet_water, 0.0..=2.0).text("再湿潤の水量"),
        );

        ui.separator();
        ui.heading("筆圧 (M1.5)");
        match self.pen.last_pressure() {
            Some(p) => {
                ui.colored_label(
                    egui::Color32::from_rgb(64, 160, 64),
                    format!("ペン接地中(筆圧 {p:.2})"),
                );
            }
            None => {
                ui.label("ペンでキャンバスに触れると筆圧が表示されます(マウスは 1.0 固定)");
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
        // 以降は物理シミュの内部係数と診断表示=制作者の味付け用ノブ。通常の描画では触らないため
        // 既定で畳んでおく(CLAUDE.md: パラメータ調整は制作者側の作業。普段はプリセットに封じ込める)。
        // 「乾燥の細部」もここへ移し、上の「乾燥 (M2)」は主ノブ(シフト・再湿潤)だけ残した
        egui::CollapsingHeader::new("調整パラメータ(味付け)")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("乾燥の細部 (M2)");
                ui.add(
                    egui::Slider::new(&mut self.params.dry_gran, 0.0..=1.0)
                        .text("焼き込み粒状感ゲート(凹部に濃く)"),
                );
                ui.add(
                    egui::Slider::new(&mut self.params.dry_edge, 0.0..=2.0)
                        .text("焼き込みエッジダークニング(縁バンド幅は M1d の値を共用)"),
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
            });

        // 診断表示(H4): デバッグ用ヒートマップ。通常描画は「通常」モード固定で十分なので畳む
        egui::CollapsingHeader::new("表示・診断 (H4)")
            .default_open(false)
            .show(ui, |ui| {
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
            });

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
    }

    /// プリセット(H3): 名前保存+一覧読込。共通 UI は NamedStore に集約(R4)
    fn preset_panel(&mut self, ui: &mut egui::Ui) {
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
    fn replay_panel(&mut self, ui: &mut egui::Ui) {
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
        // 記録直後: 名前を付けて保存(共通 NamedStore)+ そのまま試し再生
        let mut replay_now: Option<Recording> = None;
        if let Some(recording) = self.replay_ui.pending_recording.clone() {
            if let Some(status) = self.replay_ui.store.save_controls(
                ui,
                "ストローク名",
                |name| replay::save(name, &recording),
                replay::list,
            ) {
                self.status_msg = Some(status);
            }
            if ui.button("試し再生").clicked() {
                replay_now = Some(recording);
            }
        }
        if let Some(name) = self.replay_ui.store.list_rows(ui, "▶ 再生") {
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
    }

    /// シェーダー(H1): 監視ディレクトリとコンパイル状態の表示
    fn shader_status(&mut self, ui: &mut egui::Ui) {
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

        // M1.5: ペン(egui Touch、筆圧付き)を優先し、接地中はマウスを無視する
        // (egui-winit は Touch からポインタもエミュレートするため、両方を処理すると
        // 二重ストロークになる)
        let pen_events = self.pen.poll(&response);
        let events = if self.pen.is_active() || !pen_events.is_empty() {
            pen_events
        } else {
            self.mouse.poll(&response)
        };

        let mut splats: Vec<Splat> = Vec::new();
        self.apply_pointer_events(&events, rect, &mut splats);

        // H5: 記録はフレーム基準(ストローク間の待ちも再現される)
        if let Some(recorder) = &mut self.replay_ui.recorder {
            recorder.tick();
        }

        // H5: 再生中は記録済みポインタ入力を同じテンポで流し込む(手描きと合流可)
        if let Some(player) = &mut self.replay_ui.player
            && !player.advance(&mut self.params, &mut splats)
        {
            self.replay_ui.player = None;
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // H1: .wgsl が保存されたら再ビルド(失敗しても落とさない)
        if self.watcher.take_dirty() {
            self.rebuild_shaders();
        }

        egui::Panel::left("tools")
            .default_size(280.0)
            .show(ui, |ui| {
                // M2: 乾燥ボタンはスクロールの外に置き、常に見える位置に固定する
                self.dry_controls(ui);
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| self.tool_panel(ui));
            });

        self.error_overlay(ui);

        egui::CentralPanel::default().show(ui, |ui| self.canvas_ui(ui));

        // 常時シミュレーションが走るため連続再描画
        ui.ctx().request_repaint();
    }
}
