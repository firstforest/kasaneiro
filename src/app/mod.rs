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

use crate::gpu::DriedLayer;
use crate::gpu::GpuCanvas;
use crate::gpu::LineTarget;
use linehist::{LineHistory, stroke_splats};
use crate::gpu::hot_reload::{ShaderWatcher, shader_dir};
use crate::input::{MouseSource, PenSource, PointerEvent, PointerPhase};
use crate::palette_store;
use crate::preset;
use crate::replay::{self, Player, Recording};
use crate::work;
use paint_core::brush::StrokeState;
use paint_core::sim::{CANVAS_SIZE, SimParams, Splat};
use paint_core::tool::{RasterTool, Tool, WetTool};
use ui::{NamedStore, PaletteUi, PresetUi, ReplayUi, WorkUi};
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

/// 統一 Undo 履歴(M6)の1操作の種別。Ctrl+Z を「最後の操作の種別」で振り分けるため、
/// 線画(多段、`line_history`)と湿レイヤー(1段、GPU 退避)の操作順だけを時系列で持つ。
/// - `Line`: ラスタ線画1本。実データと Redo は `LineHistory` 側が多段で持つ
/// - `Wet`: 水彩ストローク1本。退避は GPU テクスチャ1組だけ=**最新の1本のみ**戻せる
///   (新しい水彩ストロークで退避が上書きされ、古い `Wet` マーカーは無効化される)
#[derive(Clone, Copy, PartialEq, Eq)]
enum UndoKind {
    Line,
    Wet,
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
    /// M7: 作品保存の UI 状態(名前入力+一覧)
    work_ui: WorkUi,
    /// H5: ストローク記録・再生の UI 状態(名前+一覧+recorder/pending/player。R4 で集約)
    replay_ui: ReplayUi,
    /// H3/H5/H6 の操作結果の表示(保存先パスやエラー)
    status_msg: Option<String>,
    /// M4.5d: 線画(鉛筆・ペン・ハイライト)の多段 Undo/Redo 履歴。
    /// 流体を通らないラスタ線画をストローク単位で決定論的に引き直す(湿レイヤーは対象外)
    line_history: LineHistory,
    /// M6: 統一 Undo 履歴。線画(多段)と湿レイヤー(1段)の操作順を時系列で持ち、
    /// Ctrl+Z を末尾の種別で振り分ける。湿レイヤーの退避実体は GpuCanvas::wet_backup
    undo_stack: Vec<UndoKind>,
    /// M5: ランタイムパレット(4スロットの色・ρ/ω/γ)。編集したら apply_palette() で GPU へ。
    /// 乾かすと現行パレットの色がそのレイヤー専用スロットへ記録される(M5c、遡って変色しない)
    palette: pigment::Palette,
    /// M5d/e: パレット・ライブラリ(保存/読込一覧)とスポイト待機の UI 状態
    palette_ui: PaletteUi,
    /// M6: ビューの拡大率。1.0=キャンバス全体がキャンバス枠に収まる、上げるほど拡大。
    /// [1.0, MAX_ZOOM] にクランプ。
    view_zoom: f32,
    /// M6: 画面中心(uv=0.5,0.5)に来るキャンバス uv(0..1)。ホイールで拡大、
    /// 中ボタン/スペース+左ドラッグでパン。回転が絡むので左上でなく中心を保持する。
    /// 回転なしのときは窓をキャンバス内に収め、回転時は中心のみ [0,1] に留める(隅は背景)
    view_center: egui::Vec2,
    /// M6: 表示回転(ラジアン、画面中心まわり)。Shift+ホイールで15°刻み、パネルで自由角。
    view_angle: f32,
}

/// M6: ビューの最大拡大率(テクセルが潰れない範囲の実用上限)
const MAX_ZOOM: f32 = 32.0;

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
            work_ui: WorkUi {
                store: NamedStore::new(work::list()),
            },
            replay_ui: ReplayUi {
                store: NamedStore::new(replay::list()),
                recorder: None,
                pending_recording: None,
                player: None,
            },
            status_msg: None,
            line_history: LineHistory::default(),
            undo_stack: Vec::new(),
            // GpuCanvas::new が既定パレットを両バッファへ書き込み済み(同じ値で開始)
            palette: pigment::Palette::default_palette(),
            palette_ui: PaletteUi {
                store: NamedStore::new(palette_store::list()),
                eyedropper: false,
            },
            view_zoom: 1.0,
            view_center: egui::vec2(0.5, 0.5),
            view_angle: 0.0,
        }
    }

    /// M6: 現在のビュー状態を display 用 uniform へ。
    /// canvas_uv = center + R(θ)·(画面uv − 0.5)·span、span = 1/zoom。
    /// CanvasCallback へ渡してフレームごとに反映する
    fn view_uniform(&self) -> crate::gpu::ViewUniform {
        let (sin_t, cos_t) = self.view_angle.sin_cos();
        crate::gpu::ViewUniform {
            center: [self.view_center.x, self.view_center.y],
            span: 1.0 / self.view_zoom,
            cos_t,
            sin_t,
            _pad: [0.0; 3],
        }
    }

    /// M6: 画面 uv からのオフセット(画面中心基準)を回転・スケールしてキャンバス uv 差分へ。
    /// screen_to_texel と zoom/pan で共有する display と同じ写像の芯
    fn view_rotate(&self, d: egui::Vec2) -> egui::Vec2 {
        let (s, c) = self.view_angle.sin_cos();
        egui::vec2(d.x * c - d.y * s, d.x * s + d.y * c)
    }

    /// M6: ビューを健全な範囲に収める。回転なしなら窓をキャンバス内に(従来挙動)、
    /// 回転時は隅がはみ出す前提で中心のみ [0,1] に留める(はみ出しは display が背景色)
    fn clamp_view(&mut self) {
        self.view_zoom = self.view_zoom.clamp(1.0, MAX_ZOOM);
        let half = 0.5 / self.view_zoom; // 窓の半幅(キャンバス uv 単位)
        let (lo, hi) = if self.view_angle == 0.0 {
            (half.min(0.5), (1.0 - half).max(0.5))
        } else {
            (0.0, 1.0)
        };
        self.view_center.x = self.view_center.x.clamp(lo, hi);
        self.view_center.y = self.view_center.y.clamp(lo, hi);
    }

    /// M6: ビューを初期状態(全体表示・回転なし)に戻す
    fn reset_view(&mut self) {
        self.view_zoom = 1.0;
        self.view_center = egui::vec2(0.5, 0.5);
        self.view_angle = 0.0;
    }

    /// M6: カーソル位置(画面座標)を中心に据えたまま拡大率を factor 倍する。
    /// カーソル下のキャンバス uv を固定して zoom を変え、center を解き直す(回転込み)
    fn zoom_at(&mut self, cursor: egui::Pos2, rect: egui::Rect, factor: f32) {
        let d = (cursor - rect.min) / rect.width().max(1.0) - egui::vec2(0.5, 0.5); // 画面中心からのオフセット
        let dr = self.view_rotate(d);
        let old_span = 1.0 / self.view_zoom;
        let anchor = self.view_center + dr * old_span; // カーソル下のキャンバス uv(固定したい点)
        self.view_zoom = (self.view_zoom * factor).clamp(1.0, MAX_ZOOM);
        let new_span = 1.0 / self.view_zoom;
        self.view_center = anchor - dr * new_span;
        self.clamp_view();
    }

    /// M6: 表示回転を delta ラジアン変える(画面中心まわり。中心・拡大率は保持)
    fn rotate_view(&mut self, delta: f32) {
        // [-π, π] に正規化(スライダー表示と往復のため)
        let mut a = self.view_angle + delta;
        let two_pi = std::f32::consts::TAU;
        a = (a + std::f32::consts::PI).rem_euclid(two_pi) - std::f32::consts::PI;
        self.view_angle = a;
        self.clamp_view();
    }

    /// M6: 画面座標(キャンバス枠内)をキャンバステクセル座標へ写す。ビュー変換(パン/ズーム/回転)込み。
    /// texel = (center + R(θ)·(画面uv − 0.5)·span) × CANVAS_SIZE。描画・スポイトの共通経路
    fn screen_to_texel(&self, pos: egui::Pos2, rect: egui::Rect) -> egui::Vec2 {
        let d = (pos - rect.min) / rect.width().max(1.0) - egui::vec2(0.5, 0.5);
        let cuv = self.view_center + self.view_rotate(d) * (1.0 / self.view_zoom);
        cuv * CANVAS_SIZE as f32
    }

    /// M5: 現行パレット(self.palette)を GPU へ反映する。色・ρ/ω/γ を編集したあとに呼ぶ。
    /// physics は即時に湿シミュへ効き、色は live パレット枠だけ更新される(乾燥済みは不変=M5c)
    fn apply_palette(&mut self) {
        let mut renderer = self.render_state.renderer.write();
        if let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() {
            canvas.set_palette(&self.render_state.queue, &self.palette);
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
        // 統一 Undo 履歴も破棄(退避テクスチャは次の水彩ストロークで上書きされるので消さなくてよい)
        self.undo_stack.clear();
    }

    /// 統一 Undo(M6): 末尾の操作種別で振り分ける(Ctrl+Z)。線を描いた直後は線が、
    /// 水彩ストロークの直後は湿レイヤーが戻る。ボタン・キーの両方からここを通す
    fn undo(&mut self) {
        match self.undo_stack.last().copied() {
            Some(UndoKind::Wet) => self.wet_undo(),
            Some(UndoKind::Line) => {
                self.line_undo();
            }
            None => {}
        }
    }

    /// 統一 Redo(M6): Ctrl+Shift+Z。湿レイヤーは 1 段 undo で Redo を持たないため、
    /// Redo 対象は線画のみ(新しいストロークで線画 Redo は破棄済み=順序は破綻しない)
    fn redo(&mut self) {
        self.line_redo();
    }

    /// 湿レイヤーの 1 段 undo(M6): 退避テクスチャを current へ書き戻し、Wet マーカーを外す。
    /// 退避は最新1本ぶんだけなので、これで戻せるのは直前の水彩ストローク開始時の状態
    fn wet_undo(&mut self) {
        self.run_canvas_action(|c, d, q| {
            c.restore_wet(d, q);
            Ok(())
        });
        if let Some(pos) = self.undo_stack.iter().rposition(|k| *k == UndoKind::Wet) {
            self.undo_stack.remove(pos);
        }
    }

    /// 水彩ストローク開始(Down)時に湿レイヤーを退避する(M6)。restore の読み元になる
    fn backup_wet_layer(&mut self) {
        self.run_canvas_action(|c, d, q| {
            c.backup_wet(d, q);
            Ok(())
        });
    }

    /// 乾かす / Fast Dry / 再湿潤 は湿レイヤーを別経路で書き替えるので、退避との整合が崩れる。
    /// これらの手動操作後は水彩の 1 段 undo を無効化する(M6。線画の履歴には触れない)
    fn invalidate_wet_undo(&mut self) {
        self.undo_stack.retain(|k| *k != UndoKind::Wet);
    }

    /// 線画の Undo(M4.5d): 末尾のストロークを Redo へ移し、その target テクスチャを
    /// クリアして残りのストロークを保存済みパラメータで引き直す(他の線種は無傷)。
    /// 実際に1本戻したら true(統一スタックの Line マーカーも1つ外す。M6)
    fn line_undo(&mut self) -> bool {
        let Some(stroke) = self.line_history.done.pop() else {
            return false;
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
        if let Some(pos) = self.undo_stack.iter().rposition(|k| *k == UndoKind::Line) {
            self.undo_stack.remove(pos);
        }
        true
    }

    /// 線画の Redo(M4.5d): 取り消した末尾のストロークを対象テクスチャへ再適用する。
    /// 実際に1本復元したら true(統一スタックへ Line マーカーを積む。M6)
    fn line_redo(&mut self) -> bool {
        let Some(stroke) = self.line_history.redo.pop() else {
            return false;
        };
        let target = stroke.target;
        let params = stroke.params;
        let splats = stroke_splats(&stroke);
        self.run_canvas_action(move |c, d, q| c.rasterize_line(d, q, target, &params, &splats));
        self.line_history.done.push(stroke);
        self.undo_stack.push(UndoKind::Line);
        true
    }

    /// Undo/Redo ショートカット(M4.5d/M6): Ctrl+Z / Ctrl+Shift+Z(Redo は Ctrl+Y も)。
    /// 統一履歴を通す(線画・水彩の両方)。テキスト入力中(プリセット名など)は横取りしない
    fn handle_undo_shortcuts(&mut self, ctx: &egui::Context) {
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
            self.undo();
        }
        if redo {
            self.redo();
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
    /// (同一入力での A/B 比較のため、必ず白紙から)。
    /// M5d: 記録にパレットがあれば現行パレットをそれへ切り替える(顔料を編集済みでも
    /// 「当時の色」で再生される)。無い旧記録は現行パレットのまま再生する
    fn start_replay(&mut self, recording: Recording, palette: Option<pigment::Palette>) {
        self.clear_canvas();
        self.stroke.end();
        self.painting = false;
        if let Some(palette) = palette {
            self.palette = palette;
            self.apply_palette();
        }
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
                    } else {
                        // M6: 水彩ストローク開始 → 湿レイヤーを退避(1段 undo の読み元)。
                        // このフレームの splat + シミュより前=ストローク直前の状態を捉える
                        self.backup_wet_layer();
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
                            if self.line_history.finish() {
                                self.undo_stack.push(UndoKind::Line);
                            }
                        } else {
                            // M6: 水彩ストローク確定。退避は最新1本ぶんだけなので、古い Wet
                            // マーカーを外して積み直す。新ストロークは線画 Redo も無効化する
                            self.invalidate_wet_undo();
                            self.undo_stack.push(UndoKind::Wet);
                            self.line_history.redo.clear();
                        }
                    }
                    continue;
                }
            }
            if !self.painting {
                continue;
            }
            // M6: ビュー変換込みでキャンバステクセルへ写す(ズーム中は拡大領域の座標になる)
            let px = self.screen_to_texel(ev.pos, rect);
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

    /// M5e スポイト: カーソル下1px の表示色を拾い、選択中の顔料スロット(brush_channel)へ入れる。
    /// snapshot() の読み戻し(display と同じ発色)から該当テクセルの RGB を取り出す。
    /// 表示色版なので「混ざってできた見た目の色」がそのまま顔料の基本色になる(§4 の先送り表)
    fn pick_color(&mut self, pos: egui::Pos2, rect: egui::Rect) {
        // M6: snapshot は display と同じビュー変換(パン/ズーム/回転)込みで焼くので、
        // 画面 uv でそのまま索引する(= カーソル下に見えている画素)。screen_to_texel は
        // キャンバステクセルなので拡大・回転時にはズレる(zoom=1・無回転でのみ一致)
        let su = (pos - rect.min) / rect.width().max(1.0);
        let x = ((su.x * CANVAS_SIZE as f32) as i32).clamp(0, CANVAS_SIZE as i32 - 1) as u32;
        let y = ((su.y * CANVAS_SIZE as f32) as i32).clamp(0, CANVAS_SIZE as i32 - 1) as u32;
        let data = {
            let renderer = self.render_state.renderer.read();
            match renderer.callback_resources.get::<GpuCanvas>() {
                Some(canvas) => {
                    canvas.snapshot(&self.render_state.device, &self.render_state.queue)
                }
                None => Err("キャンバスが初期化されていません".to_owned()),
            }
        };
        let data = match data {
            Ok(d) => d,
            Err(e) => {
                self.status_msg = Some(e);
                return;
            }
        };
        // snapshot は RGBA8 の行連続(bytes_per_row = CANVAS_SIZE*4、パディングなし)
        let idx = ((y * CANVAS_SIZE + x) * 4) as usize;
        let rgb = [data[idx], data[idx + 1], data[idx + 2]];
        let slot = self.params.brush_channel.min(3) as usize;
        self.palette.pigments[slot].rgb = rgb;
        self.apply_palette();
        self.status_msg = Some(format!(
            "スポイト: スロット #{} ← #{:02X}{:02X}{:02X}",
            slot + 1,
            rgb[0],
            rgb[1],
            rgb[2]
        ));
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

    /// M7: 現在の全状態(湿レイヤー・乾燥レイヤー・線画・レイヤーごとパレット・レイヤー構成・
    /// 現行パレット・SimParams)を1ファイルへ保存する。GPU から読み戻して work::save へ渡す。
    /// 描きかけ(湿った絵の具含む)から翌日続けられるようにするのが目的
    fn save_work(&mut self, name: &str) -> Result<PathBuf, String> {
        let file = {
            let renderer = self.render_state.renderer.read();
            let canvas = renderer
                .callback_resources
                .get::<GpuCanvas>()
                .ok_or("キャンバスが初期化されていません")?;
            let textures =
                canvas.export_state(&self.render_state.device, &self.render_state.queue)?;
            let layers = canvas
                .layers
                .iter()
                .map(|l| work::StoredLayer {
                    slot: l.slot,
                    visible: l.visible,
                })
                .collect();
            work::WorkFile {
                params: self.params,
                palette: self.palette.clone(),
                layers,
                textures,
            }
        };
        work::save(name, &file)
    }

    /// M7: 保存済み作品を読み込んでキャンバス全体を差し替える。
    /// テクスチャを書き戻し、レイヤー構成・パレット・パラメータを復元して「続きが描ける」状態にする
    fn load_work(&mut self, name: &str) {
        let file = match work::load(name) {
            Ok(f) => f,
            Err(e) => {
                self.status_msg = Some(e);
                return;
            }
        };
        // GPU 側(テクスチャ・レイヤー・パレット)を復元
        let result: Result<(), String> = {
            let mut renderer = self.render_state.renderer.write();
            match renderer.callback_resources.get_mut::<GpuCanvas>() {
                Some(canvas) => {
                    canvas
                        .import_state(&self.render_state.queue, &file.textures)
                        .map(|()| {
                            canvas.layers = file
                                .layers
                                .iter()
                                .map(|l| DriedLayer {
                                    slot: l.slot,
                                    visible: l.visible,
                                })
                                .collect();
                            canvas.sync_layers(&self.render_state.queue);
                            // physics(ρ/ω/γ)と live latent 枠を現行パレットで整える
                            // (latent 本体は import_state が丸ごと復元済み。同値で上書き)
                            canvas.set_palette(&self.render_state.queue, &file.palette);
                        })
                }
                None => Err("キャンバスが初期化されていません".to_owned()),
            }
        };
        if let Err(e) = result {
            self.status_msg = Some(e);
            return;
        }
        // app 側の状態を復元。読込後は履歴を破棄する(退避テクスチャ・線画履歴は旧キャンバスのもの)
        self.params = file.params;
        self.palette = file.palette;
        self.line_history.clear();
        self.undo_stack.clear();
        self.stroke.end();
        self.painting = false;
        self.status_msg = Some(format!("読込: {name}"));
    }

    /// 左パネルのスクロール内容。セクションごとのメソッドへ振り分けるだけ(R4)。
    /// 各セクションの実装は ui サブモジュール。M4.5/M5 でセクションが増えても
    /// このディスパッチャに1行足すだけで済む
    fn tool_panel(&mut self, ui: &mut egui::Ui) {
        self.brush_panel(ui);
        self.palette_panel(ui);
        self.linework_panel(ui);
        self.layers_panel(ui);
        self.tuning_panel(ui);
        self.view_panel(ui);
        self.preset_panel(ui);
        self.work_panel(ui);
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

        // M4.5d/M6: 統一 Undo/Redo(Ctrl+Z / Ctrl+Shift+Z)。線画・水彩の両方を振り分ける
        self.handle_undo_shortcuts(ui.ctx());

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
