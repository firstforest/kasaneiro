//! egui の画面構成の中核: [`PaintApp`] の状態・ライフサイクル(生成 / シェーダー再ビルド /
//! キャンバス操作 / ポインタ入力の反映 / スナップショット保存)と、eframe::App の描画エントリ。
//!
//! 各パネルの描画は [`ui`] 配下のサブモジュールに `impl PaintApp` として分割してある
//! (窮屈になったら per-file 分割する方針の実施。R4):
//! - [`ui::tools`] 相当 — 乾燥ボタン・水ブラシ(`dry_controls` / `brush_panel`)
//! - レイヤー(`layer_panel` / `layers_panel`)・調整(`tuning_panel`)
//! - プリセット/記録再生/シェーダー状態(`preset_panel` / `replay_panel` / `shader_status`)
//! - キャンバスとエラーオーバーレイ(`canvas_ui` / `error_overlay`)

mod linehist;
mod ui;

use crate::gpu::GpuCanvas;
use crate::gpu::LineTarget;
use linehist::{LineHistory, stroke_splats};
use crate::gpu::hot_reload::{ShaderWatcher, shader_dir};
use crate::input::{MouseSource, PenSource, PointerEvent, PointerPhase};
use crate::preset;
use crate::replay::{self, Player, Recording};
use paint_core::brush::StrokeState;
use paint_core::sim::{CANVAS_SIZE, SimParams, Splat};
use paint_core::tool::{RasterTool, Tool, WetTool};
use ui::{NamedStore, PresetUi, ReplayUi};
use eframe::egui;
use eframe::egui_wgpu;
use std::path::{Path, PathBuf};

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
    /// M4.5d: 線画(鉛筆・ペン・ハイライト)の多段 Undo/Redo 履歴。
    /// 流体を通らないラスタ線画をストローク単位で決定論的に引き直す(湿レイヤーは対象外)
    line_history: LineHistory,
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
            line_history: LineHistory::default(),
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

    fn clear_canvas(&mut self) {
        {
            let mut renderer = self.render_state.renderer.write();
            if let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() {
                canvas.clear(&self.render_state.queue);
            }
        }
        // 線画テクスチャは canvas.clear() がゼロにする。履歴も破棄する(M4.5d)
        self.line_history.clear();
    }

    /// 線画の Undo(M4.5d): 末尾のストロークを Redo へ移し、その target テクスチャを
    /// クリアして残りのストロークを保存済みパラメータで引き直す(他の線種は無傷)
    fn line_undo(&mut self) {
        let Some(stroke) = self.line_history.done.pop() else {
            return;
        };
        let target = stroke.target;
        self.line_history.redo.push(stroke);
        // 対象 target の残りストロークを (params, splats) に確定させてから GPU へ流す
        let strokes: Vec<(SimParams, Vec<Splat>)> = self
            .line_history
            .done
            .iter()
            .filter(|s| s.target == target)
            .map(|s| (s.params, stroke_splats(s)))
            .collect();
        self.run_canvas_action(move |c, d, q| {
            c.clear_line(q, target);
            for (params, splats) in &strokes {
                c.rasterize_line(d, q, target, params, splats)?;
            }
            Ok(())
        });
    }

    /// 線画の Redo(M4.5d): 取り消した末尾のストロークを対象テクスチャへ再適用する
    fn line_redo(&mut self) {
        let Some(stroke) = self.line_history.redo.pop() else {
            return;
        };
        let target = stroke.target;
        let params = stroke.params;
        let splats = stroke_splats(&stroke);
        self.run_canvas_action(move |c, d, q| c.rasterize_line(d, q, target, &params, &splats));
        self.line_history.done.push(stroke);
    }

    /// 線画の Undo/Redo ショートカット(M4.5d): Ctrl+Z / Ctrl+Shift+Z(Redo は Ctrl+Y も)。
    /// テキスト入力中(プリセット名など)は横取りしない
    fn handle_line_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (undo, redo) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let z = i.key_pressed(egui::Key::Z);
            let undo = cmd && !i.modifiers.shift && z;
            let redo = cmd && ((i.modifiers.shift && z) || i.key_pressed(egui::Key::Y));
            (undo, redo)
        });
        if undo {
            self.line_undo();
        }
        if redo {
            self.line_redo();
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
        // H5 の記録は流体ストロークだけを対象にする(記録は params.tool = WetTool の gpu_id を
        // 前提に再生するため)。ラスタ線画(M4.5)は line_history に別系統で履歴を持つ(M4.5d)
        let recordable = self.tool.wet().is_some();
        let line_target = self.line_target();
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
                    if let Some(recorder) = &mut self.replay_ui.recorder
                        && recordable
                    {
                        recorder.begin_stroke(self.params.brush_channel, self.params.tool);
                    }
                    // M4.5d: ラスタ線画は履歴へストロークを開始(実効パラメータをスナップショット)
                    if let Some(target) = line_target {
                        self.line_history.begin(target, self.params);
                    }
                }
                PointerPhase::Move => {}
                PointerPhase::Up => {
                    if self.painting {
                        self.painting = false;
                        self.stroke.end();
                        if let Some(recorder) = &mut self.replay_ui.recorder
                            && recordable
                        {
                            recorder.end_stroke();
                        }
                        // M4.5d: ラスタ線画のストロークを確定(Redo 履歴を破棄)
                        if line_target.is_some() {
                            self.line_history.finish();
                        }
                    }
                    continue;
                }
            }
            if !self.painting {
                continue;
            }
            let px = (ev.pos - rect.min) * scale;
            // サンプル間隔は筆圧を反映した実効半径から(細い筆入れでも隙間を作らない)。
            // ラスタ線画は鉛筆/ペン/ハイライトの独立半径を使う(brush_radius と切り離す)
            let base = self.active_base_radius();
            let spacing = (self.params.radius_at_base(base, ev.pressure) * 0.25).max(1.0);
            self.stroke
                .add_motion([px.x, px.y], ev.pressure, spacing, splats);
            // H5: 補間前の生ポインタ位置+筆圧を記録する(再生時に補間し直すため
            // ブラシ半径や筆圧マッピングを変えても同じストロークを引ける)
            if let Some(recorder) = &mut self.replay_ui.recorder
                && recordable
            {
                recorder.add_point([px.x, px.y], ev.pressure);
            }
            // M4.5d: ラスタ線画は生ポインタ点を履歴に溜める(Undo で引き直すため)
            if line_target.is_some() {
                self.line_history.push_point([px.x, px.y], ev.pressure);
            }
        }
    }

    /// 現在のツールが使うブラシ半径の基準値(筆圧前)。ラスタ線画は鉛筆/ペンの
    /// 独立半径、流体ツールは brush_radius。ストローク補間の間隔算出に使う
    fn active_base_radius(&self) -> f32 {
        match self.tool {
            Tool::Raster { kind: RasterTool::Pencil, .. } => self.params.pencil_radius,
            Tool::Raster { kind: RasterTool::Pen, .. } => self.params.pen_radius,
            Tool::Raster { kind: RasterTool::Highlight, .. } => self.params.highlight_radius,
            _ => self.params.brush_radius,
        }
    }

    /// ラスタ線画ツール(M4.5a/c)選択中の描画先。流体ツールのときは None。
    /// CanvasCallback へ渡し、Some なら splat を linesplat.wgsl へ流す
    fn line_target(&self) -> Option<LineTarget> {
        match self.tool {
            Tool::Raster { kind: RasterTool::Pencil, .. } => Some(LineTarget::Pencil),
            Tool::Raster { kind: RasterTool::Pen, .. } => Some(LineTarget::Pen),
            Tool::Raster { kind: RasterTool::Highlight, .. } => Some(LineTarget::Highlight),
            _ => None,
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
    /// 各セクションの実装は ui サブモジュール。M4.5/M5 でセクションが増えても
    /// このディスパッチャに1行足すだけで済む
    fn tool_panel(&mut self, ui: &mut egui::Ui) {
        self.brush_panel(ui);
        self.linework_panel(ui);
        self.layers_panel(ui);
        self.tuning_panel(ui);
        self.preset_panel(ui);
        self.replay_panel(ui);
        self.shader_status(ui);
    }
}

impl eframe::App for PaintApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // H1: .wgsl が保存されたら再ビルド(失敗しても落とさない)
        if self.watcher.take_dirty() {
            self.rebuild_shaders();
        }

        // M4.5d: 線画の Undo/Redo(Ctrl+Z / Ctrl+Shift+Z)
        self.handle_line_shortcuts(ui.ctx());

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
