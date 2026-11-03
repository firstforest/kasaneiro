//! egui の画面構成の中核: [`PaintApp`] の状態・ライフサイクル(生成 / シェーダー再ビルド /
//! キャンバス操作 / ポインタ入力の反映 / スナップショット保存)と、eframe::App の描画エントリ。
//!
//! 各パネルの描画は [`ui`] 配下のサブモジュールに `impl PaintApp` として分割してある:
//! - [`ui::tools`] 相当 — 乾燥ボタン・元に戻す・水ブラシ・右パネルの色まわり(`dry_controls` / `undo_controls` / `brush_panel` / `wet_palette_panel`)
//! - レイヤー(`layer_panel` / `layers_panel`)・調整(`tuning_panel`)
//! - プリセット/記録再生/シェーダー状態(`preset_panel` / `replay_panel` / `shader_status`)
//! - キャンバスとエラーオーバーレイ(`canvas_ui` / `error_overlay`)

mod linehist;
mod ui;

use crate::capture::{self, Capture, Closing};
use crate::gpu::DriedLayer;
use crate::gpu::GpuCanvas;
use crate::gpu::LineTarget;
use crate::gpu::hot_reload::{ScreenshotWatcher, ShaderWatcher, screenshots_dir, shader_dir};
use crate::input::{MouseSource, PenSource, PointerEvent, PointerPhase};
use crate::palette_store;
use crate::pigment_store;
use crate::preset;
use crate::replay::{self, Player, Recording};
use crate::work;
use eframe::egui;
use eframe::egui_wgpu;
use linehist::{LineHistory, line_smoother, stroke_splats};
use paint_core::brush::{LineSmoother, StrokeState};
use paint_core::clock::SimClock;
use paint_core::load::{BrushClock, BrushFrame, BrushLoad};
use paint_core::sim::{DEFAULT_CANVAS_SIZE, SimParams, Splat};
use paint_core::tool::{RasterTool, Tool, WetTool};
use std::path::PathBuf;
use ui::{Eyedropper, NamedStore, PaletteUi, PresetUi, ReplayUi, WorkUi};

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

/// UI のアクティブレイヤー(右のレイヤーパネルで選択)。レイヤーごとに使えるツールが
/// 決まっているため、選択がそのまま「描画先」と「左パネルに出すツール群」を決める。
/// 並びは合成順(上=手前): ハイライト → ペン → 鉛筆 → 水彩(湿)→ 乾燥 → 紙。
/// 乾燥レイヤーは焼き込み済みで編集不可(選択できるが描画はブロックし、案内を出す)
#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveLayer {
    /// 白ハイライト(最上段の線画テクスチャ。M4.5c)
    Highlight,
    /// 清書ペン(M4.5a。水の境界にもなる)
    Pen,
    /// 下書き鉛筆(M4.5a)
    Pencil,
    /// 水彩の湿レイヤー(流体シミュの描画先)
    Wet,
    /// 乾燥レイヤー(`GpuCanvas::layers` のインデックス)。編集不可
    Dried(usize),
}

/// 上部「ファイル」メニューから開くモーダルの種別。None=どれも開いていない(file_menu.rs)。
/// 破壊操作(新規キャンバス・全部消す)はモーダル内の明示ボタンで確認する(旧2度押し機構を置換)。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum FileModal {
    /// 作品の保存+開く(統合モーダル)
    Work,
    /// 同名作品への上書き保存の確認(対象名は WorkUi::pending)
    WorkOverwrite,
    /// 作品の読込で現在のキャンバスを破棄する確認(対象名は WorkUi::pending)
    WorkLoad,
    /// 設定プリセット(SimParams)の保存+開く(統合モーダル)
    Preset,
    /// 新規キャンバス(サイズ選択+確認)
    NewCanvas,
    /// 全部消す(確認)
    Clear,
    /// かさねいろについて(バージョン・ライセンス表示)
    About,
}

pub struct PaintApp {
    render_state: egui_wgpu::RenderState,
    params: SimParams,
    /// 選択中ツール。トップレベルの分岐が描画経路の分岐。wet ツールは
    /// `WetTool::gpu_id()` を `params.tool` へ同期して GPU の splat 分岐に渡す。
    /// ラスタツール(M4.5)は流体経路に流れない
    tool: Tool,
    /// 選択中のレイヤー(UI)。右のレイヤーパネルで選び、左パネルはこのレイヤーの
    /// ツールだけを出す。ツールとの同期は `select_layer` が担う
    active_layer: ActiveLayer,
    /// 水彩レイヤーで最後に使っていたツール。別レイヤーへ移って戻ったときに復元する
    last_wet_tool: WetTool,
    /// スポイト(I キー)を押しっぱなしで一時待機にしている間 true。離したら待機解除する
    /// (ペンタブレットのキー割当を想定した spring-loaded。UI のトグルとは独立に動く)
    eyedropper_hold: bool,
    /// 消す(E キー)を押しっぱなしで「今いる描画レイヤーの消しゴム」に一時切替している間の、
    /// 離したとき戻す元ツール(水彩=Wet(Erase) / 線画=Raster{eraser:true} / 乾燥=無反応)
    erase_hold: Option<Tool>,
    stroke: StrokeState,
    /// 描いている線画ストロークの手ぶれ補正(清書ペンのときだけ。linehist::line_smoother)
    line_smoother: Option<LineSmoother>,
    /// 塗る筆の中身(水と4色の顔料)。正典は GPU の筆バッファで、ここはフレーム頭に読み戻した値
    /// (描いている間は数フレーム遅れる)。ツールバーの色・水・洗うは edit_brush で
    /// 最新の値を待ってから変え、GPU へその場で書き戻す
    brush_load: BrushLoad,
    /// このフレームに GPU の筆へ渡す指示(穂先の残りの予測・補充の時間。再生中は記録した中身への
    /// 置き換えも)。キャンバスのコールバックを組むときに取り出す
    brush_frame: BrushFrame,
    /// 記録の再生前の筆の中身(再生は記録した中身で塗るので、止めたら戻す)
    replay_saved_brush: Option<BrushLoad>,
    /// 腹から穂先へ補充する時間(刻み)を、塗るフレームまで溜めておく
    brush_clock: BrushClock,
    /// 自動補充で戻す中身 = 最後に色・水・洗うで作った時点の中身。eframe storage に永続化
    brush_refill: BrushLoad,
    /// 自動補充: 筆を下ろすたびに brush_refill へ戻す。off(既定)なら前のストロークの残りから続き、
    /// 乾きかけの筆で濡れた色を吸える。eframe storage に永続化
    auto_refill: bool,
    /// 押している最中のスウォッチ(0〜3 = 色、4 = 水)。押し始めの判定と押し続けの取り込みに使う
    swatch_held: Option<usize>,
    /// 直近のストローク位置と筆圧(テクセル座標)。置いたまま動かないフレームの
    /// splat をここに積む(hold_still)。Up でクリア
    feed_pos: Option<([f32; 2], f32)>,
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
    /// H6: 速度倍率(1刻み=1/60 秒あたりのシミュレーションステップ数)
    steps_per_tick: u32,
    /// シミュレーションの固定刻み時計(フレームの経過時間を刻み数に変える)
    sim_clock: SimClock,
    /// H3: プリセットの UI 状態(名前入力+一覧)
    preset_ui: PresetUi,
    /// M7: 作品保存の UI 状態(名前入力+一覧)
    work_ui: WorkUi,
    /// H5: ストローク記録・再生の UI 状態(名前+一覧+recorder/pending/player)
    replay_ui: ReplayUi,
    /// H3/H5/H6 の操作結果の表示(保存先パスやエラー)
    status_msg: Option<String>,
    /// 上部「ファイル」メニューから開いているモーダル。None=どれも開いていない(file_menu.rs)
    file_modal: Option<FileModal>,
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
    /// M8: 現在のキャンバス1辺(GpuCanvas::size の写し。座標変換・保存が毎フレーム使うため
    /// renderer ロックなしで読めるよう app 側にも持つ)。変更は recreate_canvas 経由のみ
    canvas_size: u32,
    /// M8: UI のサイズ選択(コンボボックス)。「新規キャンバス」で canvas_size へ反映される
    pending_canvas_size: u32,
    /// H1/M8: シェーダーディレクトリ(GpuCanvas の作り直しで再利用する)
    shader_dir: PathBuf,
    /// UI スクショ(AI レビュー用): egui の Screenshot コマンドを送った後、
    /// 次フレーム以降に届く `Event::Screenshot` を待っている間 true。
    /// 受け取ったら固定パス screenshots/ui-latest.png へ上書き保存する
    screenshot_pending: bool,
    /// UI スクショ(AI レビュー用): screenshots/request-shot の作成/変更を監視し、
    /// AI(外部 Bash)からの撮影指示をフレームループで拾う。ボタン撮影とは別経路
    shot_watcher: ScreenshotWatcher,
    /// 解析用キャプチャ(`--capture` で起動したときだけ Some)。起動直後からのストローク記録と、
    /// 終了時の書き出しの進み具合を持つ
    capture: Option<Capture>,
    /// 開発モード(UI 二層化)。off=通常ユーザー向け最小 UI / on=味付け・診断・シミュ制御・
    /// 記録再生・シェーダー状態を露出。開発機能は削除でなくこのトグルの裏へ退避する。
    /// eframe storage に永続化(前回の状態で起動。初回のみ off)
    dev_mode: bool,
    /// 初回体験ガイド(F15): まだ一度も描いていない間だけ空キャンバスにヒントを出す。
    /// 最初の一筆(apply_pointer_events の Down)・作品の読込(load_work)・再生開始
    /// (start_replay)で true になり、以降ガイドは消える
    has_painted: bool,
}

/// M6: ビューの最大拡大率(テクセルが潰れない範囲の実用上限)
const MAX_ZOOM: f32 = 32.0;

/// 筆へのスポイトで顔料を平均する窓の半径(テクセル)。紙の斑・粒状感で1テクセルの比が揺れるのをならす
const EYEDROPPER_RADIUS: u32 = 3;
/// 筆へのスポイトで「絵の具がある」とみなす顔料の合計(1テクセルあたり)。被覆率 1−exp(−総濃度) で 0.5% 程度
const EYEDROPPER_MIN_PIGMENT: f32 = 0.005;
/// 筆へのスポイトで拾う色の最小の割合。これ未満の色は落とす
const EYEDROPPER_MIN_SHARE: f32 = 0.02;

impl PaintApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_capture(cc, None)
    }

    /// `capture_dir` があれば、起動直後から終了までを解析用にそこへ記録する([`crate::capture`])
    pub fn with_capture(cc: &eframe::CreationContext<'_>, capture_dir: Option<PathBuf>) -> Self {
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
            DEFAULT_CANVAS_SIZE,
        );
        let shader_error = canvas.rebuild_pipelines(&render_state.device).err();
        if let Some(e) = &shader_error {
            log::error!("WGSL の初回ビルドに失敗: {e}");
        }

        // F11: 開発モードを前回の状態で復元(eframe storage。初回=キー無し=通常モード off)
        let dev_mode = cc
            .storage
            .and_then(|s| s.get_string("dev_mode"))
            .map(|v| v == "true")
            .unwrap_or(false);
        // 色の管理: 前回終了時のパレット(4色+性質)を復元する。スライダー調整と同じく
        // 「作った色がセッション限りで消える」のを防ぐ(JSON が壊れていれば既定へフォールバック)。
        // 一緒に、そのパレットの元になった保存名も復元する(「変更あり」表示の基準)
        let palette = cc
            .storage
            .and_then(|s| s.get_string("palette"))
            .and_then(|json| serde_json::from_str::<pigment::Palette>(&json).ok())
            .unwrap_or_else(pigment::Palette::default_palette);
        let palette_name = cc
            .storage
            .and_then(|s| s.get_string("palette_name"))
            .unwrap_or_default();
        let auto_refill = cc
            .storage
            .and_then(|s| s.get_string("auto_refill"))
            .map(|v| v == "true")
            .unwrap_or(false);
        // 筆の中身(自動補充の戻り先)を前回終了時のまま復元する。初回は空の筆
        let brush_refill = cc
            .storage
            .and_then(|s| s.get_string("brush_load"))
            .and_then(|json| serde_json::from_str::<BrushLoad>(&json).ok())
            .unwrap_or_default();
        // GpuCanvas::new は既定パレットと空の筆で初期化するので、復元したパレットと筆の中身を
        // 書き込んでから挿す
        canvas.set_palette(&render_state.queue, &palette);
        canvas.set_brush(&render_state.queue, brush_refill);
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(canvas);

        let mut app = Self {
            render_state,
            params: SimParams::default(),
            tool: Tool::Wet(WetTool::Paint),
            active_layer: ActiveLayer::Wet,
            last_wet_tool: WetTool::Paint,
            eyedropper_hold: false,
            erase_hold: None,
            stroke: StrokeState::default(),
            line_smoother: None,
            brush_load: brush_refill,
            brush_frame: BrushFrame::default(),
            replay_saved_brush: None,
            brush_clock: BrushClock::default(),
            brush_refill,
            auto_refill,
            swatch_held: None,
            feed_pos: None,
            pen: PenSource::default(),
            mouse: MouseSource,
            painting: false,
            watcher: ShaderWatcher::new(&dir),
            shader_error,
            paused: false,
            step_once: false,
            steps_per_tick: 1,
            sim_clock: SimClock::default(),
            preset_ui: PresetUi {
                store: NamedStore::new(preset::list()),
            },
            work_ui: WorkUi {
                store: NamedStore::new(work::list()),
                pending: None,
            },
            replay_ui: ReplayUi {
                store: NamedStore::new(replay::list()),
                recorder: None,
                pending_recording: None,
                player: None,
                saved_channel: None,
                video: None,
            },
            status_msg: None,
            file_modal: None,
            line_history: LineHistory::default(),
            undo_stack: Vec::new(),
            // 前回終了時のパレットで開始(上で復元済み。GPU 側も set_palette で同期済み)
            palette,
            palette_ui: PaletteUi {
                store: {
                    // 名前欄=「いま使っているパレットの保存名」。前回の名前で始める
                    let mut store = NamedStore::new(palette_store::list());
                    store.name = palette_name;
                    store
                },
                eyedropper: None,
                // 色ライブラリ・パレット一覧は左パネルに常時表示するので起動時に読む
                // (以降は保存時と ↻ ボタンで更新)
                pigment_cache: pigment_store::load_all(),
                palette_cache: palette_store::load_all(),
                color_modal: false,
            },
            view_zoom: 1.0,
            view_center: egui::vec2(0.5, 0.5),
            view_angle: 0.0,
            canvas_size: DEFAULT_CANVAS_SIZE,
            pending_canvas_size: DEFAULT_CANVAS_SIZE,
            shader_dir: dir,
            screenshot_pending: false,
            shot_watcher: ScreenshotWatcher::new(&screenshots_dir()),
            capture: None,
            dev_mode,
            has_painted: false,
        };
        if let Some(dir) = capture_dir {
            app.status_msg = Some(format!("解析用に記録中: {}", dir.display()));
            app.capture = Some(Capture::new(dir, &app.params));
        }
        app
    }

    /// M8: キャンバスを指定サイズで作り直す(現在の絵・履歴・ビューは破棄)。
    /// テクスチャ・タイルバッファ・bind group が全てサイズ依存のため、GpuCanvas を丸ごと
    /// 生成し直して callback_resources を差し替える(部分的な作り直しより単純で安全。
    /// 旧キャンバスは差し替えで drop され VRAM が返る)。現行パレット・SimParams は引き継ぐ
    fn recreate_canvas(&mut self, size: u32) {
        // 筆の中身は旧キャンバスの GPU にあるので、確定させてから新キャンバスへ書き直す
        self.sync_brush();
        let mut canvas = GpuCanvas::new(
            &self.render_state.device,
            &self.render_state.queue,
            self.render_state.target_format,
            self.shader_dir.clone(),
            size,
        );
        self.shader_error = canvas.rebuild_pipelines(&self.render_state.device).err();
        // 新キャンバスは既定パレットで生成されるので、現行パレットを書き直して引き継ぐ
        canvas.set_palette(&self.render_state.queue, &self.palette);
        canvas.set_brush(&self.render_state.queue, self.brush_load);
        self.render_state
            .renderer
            .write()
            .callback_resources
            .insert(canvas);
        self.canvas_size = size;
        self.pending_canvas_size = size;
        // 旧キャンバス由来の状態は全て無効: 履歴・ストローク・ビューを初期化し、
        // 消えた乾燥レイヤーを選択していたら水彩へ戻す
        self.line_history.clear();
        self.undo_stack.clear();
        self.stroke.end();
        self.painting = false;
        self.reset_view();
        if matches!(self.active_layer, ActiveLayer::Dried(_)) {
            self.select_layer(ActiveLayer::Wet);
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
    /// texel = (center + R(θ)·(画面uv − 0.5)·span) × キャンバス1辺。描画・スポイトの共通経路
    fn screen_to_texel(&self, pos: egui::Pos2, rect: egui::Rect) -> egui::Vec2 {
        let d = (pos - rect.min) / rect.width().max(1.0) - egui::vec2(0.5, 0.5);
        let cuv = self.view_center + self.view_rotate(d) * (1.0 / self.view_zoom);
        cuv * self.canvas_size as f32
    }

    /// レイヤー選択(右のレイヤーパネル)。レイヤーごとにツール系統が決まっているため、
    /// 選択に合わせてツールを切り替える(水彩は最後に使っていたツールへ戻し、
    /// 消しゴム状態は線画レイヤー間で引き継ぐ)。乾燥レイヤーはツールなし(描画はブロック)
    fn select_layer(&mut self, layer: ActiveLayer) {
        self.active_layer = layer;
        // 筆へのスポイトは筆の中身を変えて塗るツールにするので、水彩レイヤー以外では待機を解く
        if layer != ActiveLayer::Wet && self.palette_ui.eyedropper == Some(Eyedropper::Brush) {
            self.palette_ui.eyedropper = None;
        }
        // E キー保持中の一時消しゴムは元ツールへ戻してから切り替える(保持中の元ツールは
        // 前のレイヤーのものなので、そのまま残すとキーを離したとき別レイヤーのツールが復元される)
        if let Some(prev) = self.erase_hold.take() {
            self.tool = prev;
        }
        let eraser = matches!(self.tool, Tool::Raster { eraser: true, .. });
        match layer {
            ActiveLayer::Wet => self.tool = Tool::Wet(self.last_wet_tool),
            ActiveLayer::Pencil => {
                self.tool = Tool::Raster {
                    kind: RasterTool::Pencil,
                    eraser,
                };
            }
            ActiveLayer::Pen => {
                self.tool = Tool::Raster {
                    kind: RasterTool::Pen,
                    eraser,
                };
            }
            ActiveLayer::Highlight => {
                self.tool = Tool::Raster {
                    kind: RasterTool::Highlight,
                    eraser,
                };
            }
            ActiveLayer::Dried(_) => {} // ツールは据え置き(canvas 側で描画をブロックする)
        }
    }

    /// 乾燥レイヤー選択中は描画不可(焼き込みは一方通行)。
    /// canvas がポインタ入力をストロークへ流す前に見る
    fn drawing_locked(&self) -> bool {
        matches!(self.active_layer, ActiveLayer::Dried(_))
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
            self.shader_error = canvas.rebuild_pipelines(&self.render_state.device).err();
        }
    }

    fn clear_canvas(&mut self) {
        {
            let mut renderer = self.render_state.renderer.write();
            if let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() {
                canvas.clear(&self.render_state.device, &self.render_state.queue);
            }
        }
        // 線画テクスチャは canvas.clear() がゼロにする。履歴も破棄する(M4.5d)
        self.line_history.clear();
        // 統一 Undo 履歴も破棄(退避テクスチャは次の水彩ストロークで上書きされるので消さなくてよい)
        self.undo_stack.clear();
    }

    /// 統一 Undo(M6): 末尾の操作種別で振り分ける(Ctrl+Z)。線を描いた直後は線が、
    /// 水彩ストロークの直後は湿レイヤーが戻る。ボタン・キーの両方からここを通す。
    /// ストローク中は履歴とテクスチャが食い違うので何もしない
    fn undo(&mut self) {
        if self.painting {
            return;
        }
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
        if self.painting {
            return;
        }
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

    /// GPU の筆の中身を待って brush_load に取り込む。CPU で中身を変える前・記録に残す前に呼ぶ
    fn sync_brush(&mut self) {
        let mut synced = None;
        self.run_canvas_action(|c, d, q| {
            synced = Some(c.sync_brush(d, q)?);
            Ok(())
        });
        if let Some(load) = synced {
            self.brush_load = load;
        }
    }

    /// 筆の中身を置き換える。GPU のバッファへその場で書く(次の submit の前に効く)。
    /// フレームのコールバックに載せると、egui が UI を組み直して捨てたパスの分が届かないため
    fn set_brush(&mut self, load: BrushLoad) {
        self.brush_load = load;
        self.run_canvas_action(|c, _, q| {
            c.set_brush(q, load);
            Ok(())
        });
    }

    /// 筆の中身を CPU で変える(色を取る・水に浸す・洗う)。最新の値を待ってから変えて書き戻す
    fn edit_brush(&mut self, edit: impl FnOnce(&mut BrushLoad)) {
        self.sync_brush();
        let mut load = self.brush_load;
        edit(&mut load);
        self.set_brush(load);
    }

    /// フレーム頭: 表示用に GPU の筆の中身(最後に読み戻した値)を取り込む
    fn pull_brush(&mut self) {
        let renderer = self.render_state.renderer.read();
        if let Some(canvas) = renderer.callback_resources.get::<GpuCanvas>() {
            self.brush_load = canvas.brush_mirror();
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
            c.clear_line(d, q, target);
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

    /// スポイト・消すをキーに割り当てる spring-loaded ショートカット(ペンタブレットのキー用)。
    /// 押している間だけ一時的に切り替え、離すと元へ戻す:
    /// - **I** = 筆へのスポイト(押している間だけ待機。離すと解除)。筆の中身を変えるので水彩レイヤーだけで効く
    /// - **E** = いま描いているレイヤーに応じた消しゴム(押している間だけ。離すと元ツール):
    ///   水彩レイヤー=`Wet(Erase)`(完全消去)/ 線画レイヤー(鉛筆・ペン・ハイライト)=そのレイヤーの
    ///   消しゴム(`Raster { eraser: true }`)。乾燥レイヤー選択中は描画不可なので何もしない
    ///
    /// ペンタブレット側はキーを I / E に割り当てる(ドライバ設定)。修飾キーのように「押している間ずっと」
    /// キーを送る割当を推奨。テキスト入力中(名前欄など)はキーを奪わず、保持中の一時切替は解除する。
    /// `ui()` 冒頭(パネル描画前)で呼ぶので、設定した self.tool / 待機フラグは同フレームで反映される
    /// (線画は raster_tool_header が self.tool から eraser を読み直して line_eraser に同期する)。
    fn handle_tool_shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let (i_down, e_down) = ctx.input(|i| {
            (
                !typing && i.key_down(egui::Key::I),
                !typing && i.key_down(egui::Key::E),
            )
        });

        // スポイト(I): 押している間だけ筆へのスポイト待機にする(離すと解除)。
        // 待機中にキャンバスをクリックすると canvas.rs が拾って一旦待機を落とすが、
        // 押しっぱなしなら次フレームでここが再び立てるので連続で拾える
        if i_down && self.active_layer == ActiveLayer::Wet {
            self.palette_ui.eyedropper = Some(Eyedropper::Brush);
            self.eyedropper_hold = true;
        } else if self.eyedropper_hold {
            self.palette_ui.eyedropper = None;
            self.eyedropper_hold = false;
        }

        // 消す(E): 押している間だけ「今いる描画レイヤーの消しゴム」へ一時切替(離すと元ツール)。
        // 元ツールを erase_hold に退避し、レイヤー種別で消しゴムの実体を選ぶ。
        // 乾燥レイヤー(Dried)は描画不可なので対象外(退避もしない=E は無反応)。
        let erasing = e_down
            && match self.active_layer {
                ActiveLayer::Wet => {
                    // 水彩=完全消去。params.tool も直接同期し、パネル非表示でも効くようにする
                    self.erase_hold.get_or_insert(self.tool);
                    self.tool = Tool::Wet(WetTool::Erase);
                    self.params.tool = WetTool::Erase.gpu_id();
                    true
                }
                ActiveLayer::Pencil | ActiveLayer::Pen | ActiveLayer::Highlight => {
                    // 線画=そのレイヤーの消しゴム。raster_tool_header が self.tool から
                    // eraser を読み直して line_eraser を同期するので、ここは self.tool を立てるだけ
                    self.erase_hold.get_or_insert(self.tool);
                    if let Tool::Raster { kind, .. } = self.tool {
                        self.tool = Tool::Raster { kind, eraser: true };
                    }
                    true
                }
                ActiveLayer::Dried(_) => false,
            };
        if !erasing && let Some(prev) = self.erase_hold.take() {
            self.tool = prev;
            if let Some(wt) = self.tool.wet() {
                self.params.tool = wt.gpu_id();
            }
        }
    }

    /// M2 の手動アクション(乾かす / Fast Dry / 再湿潤)を GpuCanvas 上で実行する共通経路。
    /// 失敗(シェーダー未ビルド・レイヤー上限)はステータス表示に流す。成功したら true
    fn run_canvas_action(
        &mut self,
        action: impl FnOnce(
            &mut GpuCanvas,
            &egui_wgpu::wgpu::Device,
            &egui_wgpu::wgpu::Queue,
        ) -> Result<(), String>,
    ) -> bool {
        let result = {
            let mut renderer = self.render_state.renderer.write();
            match renderer.callback_resources.get_mut::<GpuCanvas>() {
                Some(canvas) => action(canvas, &self.render_state.device, &self.render_state.queue),
                None => Err("キャンバスが初期化されていません".to_owned()),
            }
        };
        match result {
            Ok(()) => true,
            Err(e) => {
                self.status_msg = Some(e);
                false
            }
        }
    }

    /// H5: キャンバスをリセットして記録済みストロークの再生を始める
    /// (同一入力での A/B 比較のため、必ず白紙から)。
    /// M5d: 現行パレットを記録時のパレットへ切り替える(顔料を編集済みでも
    /// 「当時の色」で再生される)
    fn start_replay(&mut self, recording: Recording, palette: pigment::Palette) {
        self.clear_canvas();
        self.stroke.end();
        self.painting = false;
        // 記録は水彩ストロークなので水彩レイヤーで再生する(線画レイヤー選択中だと
        // line_target が立ち、splat が線画へ流れる)
        if self.active_layer != ActiveLayer::Wet {
            self.select_layer(ActiveLayer::Wet);
        }
        self.palette = palette;
        self.apply_palette();
        // 再生前の顔料スロットを退避(再生終了時に stop_replay が戻す)。
        // 再生中の再開でも最初の値を保つよう、既に退避済みなら上書きしない
        self.replay_ui
            .saved_channel
            .get_or_insert(self.params.brush_channel);
        // 再生は記録した中身で塗るので、いまの筆を退避して止めたら戻す
        if self.replay_saved_brush.is_none() {
            self.sync_brush();
            self.replay_saved_brush = Some(self.brush_load);
        }
        self.replay_ui.player = Some(Player::new(recording));
        // 再生で絵が載るので初回ガイドは出さない
        self.has_painted = true;
    }

    /// 再生を止め、再生前に選択していた顔料スロットへ戻す。
    /// Player::advance が params.brush_channel を記録値で上書きするため、戻さないと
    /// 再生後に選択顔料が最後のストロークの色へ飛ぶ(params.tool は毎フレーム
    /// self.tool から再同期されるので復元不要 — tools.rs)。
    fn stop_replay(&mut self) {
        self.replay_ui.player = None;
        if let Some(load) = self.replay_saved_brush.take() {
            self.set_brush(load);
        }
        if let Some(channel) = self.replay_ui.saved_channel.take() {
            self.params.brush_channel = channel;
        }
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
                    self.has_painted = true; // F15: 最初の一筆で初回ガイドを消す
                    self.stroke.begin();
                    // 自動補充: 最後に作った筆の中身へ戻してから描き始める
                    if self.auto_refill {
                        self.set_brush(self.brush_refill);
                    }
                    // H5: 記録はストローク単位。そのとき選ばれていた顔料スロット・ツール・筆の中身も残す
                    // (自動補充しないときは前のストロークの残りから始まるので、GPU の最新の値を待つ)
                    if (self.replay_ui.recorder.is_some() || self.capture.is_some()) && recordable {
                        if !self.auto_refill {
                            self.sync_brush();
                        }
                        let load = self.brush_load;
                        let (channel, tool) = (self.params.brush_channel, self.params.tool);
                        for recorder in self.recorders() {
                            recorder.begin_stroke(channel, tool, Some(load));
                        }
                        let name = format!("{:?}", self.tool);
                        if let Some(c) = &mut self.capture {
                            c.stroke_begin(&name, channel, Some(&load));
                        }
                    }
                    // M4.5d: ラスタ線画は履歴へストロークを開始(実効パラメータをスナップショット)
                    self.line_smoother = line_target.and_then(|t| line_smoother(t, &self.params));
                    if let Some(target) = line_target {
                        self.line_history.begin(target, self.params);
                        if target == LineTarget::Pen {
                            self.run_canvas_action(|c, d, q| {
                                c.snapshot_pen_base(d, q);
                                Ok(())
                            });
                        }
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
                        // 手ぶれ補正で遅れているペン先を終点まで描き切る
                        if let Some(mut smoother) = self.line_smoother.take() {
                            let mut points = Vec::new();
                            smoother.finish(&mut points);
                            let base = self.active_base_radius();
                            let params = &self.params;
                            for (pos, pressure) in points {
                                self.stroke.add_motion(
                                    pos,
                                    pressure,
                                    |p| params.splat_spacing(base, p),
                                    splats,
                                );
                            }
                        }
                        self.stroke.end();
                        self.feed_pos = None;
                        if recordable {
                            for recorder in self.recorders() {
                                recorder.end_stroke();
                            }
                            if let Some(c) = &self.capture {
                                c.stroke_end(&self.brush_load);
                            }
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
            let params = &self.params;
            let start = splats.len();
            let spacing = |p| params.splat_spacing(base, p);
            if let Some(smoother) = &mut self.line_smoother {
                let mut points = Vec::new();
                smoother.push([px.x, px.y], ev.pressure, &mut points);
                for (pos, pressure) in points {
                    self.stroke.add_motion(pos, pressure, spacing, splats);
                }
            } else {
                self.stroke
                    .add_motion([px.x, px.y], ev.pressure, spacing, splats);
            }
            if self.tool == Tool::Wet(WetTool::Paint) {
                self.brush_frame.drag(&mut splats[start..], &self.params);
            }
            // 置いたまま動かないフレームの splat をここに積む(hold_still)
            self.feed_pos = Some(([px.x, px.y], ev.pressure));
            // H5: 補間前の生ポインタ位置+筆圧を記録する(再生時に補間し直すため
            // ブラシ半径や筆圧マッピングを変えても同じストロークを引ける)
            if recordable {
                for recorder in self.recorders() {
                    recorder.add_point([px.x, px.y], ev.pressure);
                }
            }
            // M4.5d: ラスタ線画は生ポインタ点を履歴に溜める(Undo で引き直すため)
            if line_target.is_some() {
                self.line_history.push_point([px.x, px.y], ev.pressure);
            }
        }
    }

    /// 塗る筆(Wet::Paint)を置いたまま動かさないフレームでも splat を1つ積む(`Splat.feed` = 1。初速なし)。
    /// 筆と紙の受け渡しは動いているときと同じ式なので、紙と釣り合うまで色水が出続け、毛細管拡散が
    /// 外へ運ぶ(置いたまま広がる)。洗った空の筆なら、置いている間ずっと濡れた所の色水を吸う。
    /// 動いている(splat が出た)フレームは積まない
    pub(in crate::app) fn hold_still(&mut self, splats: &mut Vec<Splat>) {
        if !self.painting || self.tool != Tool::Wet(WetTool::Paint) || !splats.is_empty() {
            return;
        }
        let Some((pos, pressure)) = self.feed_pos else {
            return;
        };
        let start = splats.len();
        splats.push(Splat::feed_at(pos, pressure));
        self.brush_frame.drag(&mut splats[start..], &self.params);
    }

    /// 現在のツールが使うブラシ半径の基準値(筆圧前)。ラスタ線画は鉛筆/ペン/ハイライトごとに
    /// 描く太さと消しゴムの太さを別に持ち(SimParams::line_radius)、流体ツールは brush_radius。
    /// ストローク補間の間隔算出とカーソル表示に使う
    fn active_base_radius(&self) -> f32 {
        match self.tool {
            Tool::Raster { kind, eraser } => self.params.line_radius(kind.line_mode(), eraser),
            _ => self.params.brush_radius,
        }
    }

    /// F18: Ctrl+Alt+ドラッグでのブラシ半径調整。現在のツールの半径(テクセル)へ delta を
    /// 加算し、各スライダーと同じ 1〜64 にクランプする。active_base_radius と同じ対応で
    /// 書き込み先を選ぶ(線画の消しゴム中は消しゴムの太さが変わる)
    fn adjust_active_radius(&mut self, delta: f32) {
        let r = match self.tool {
            Tool::Raster { kind, eraser } => self.params.line_radius_mut(kind.line_mode(), eraser),
            _ => &mut self.params.brush_radius,
        };
        *r = (*r + delta).clamp(1.0, 64.0);
    }

    /// ラスタ線画ツール(M4.5a/c)選択中の描画先。流体ツールのときは None。
    /// CanvasCallback へ渡し、Some なら splat を linesplat.wgsl へ流す
    fn line_target(&self) -> Option<LineTarget> {
        match self.tool {
            Tool::Raster {
                kind: RasterTool::Pencil,
                ..
            } => Some(LineTarget::Pencil),
            Tool::Raster {
                kind: RasterTool::Pen,
                ..
            } => Some(LineTarget::Pen),
            Tool::Raster {
                kind: RasterTool::Highlight,
                ..
            } => Some(LineTarget::Highlight),
            _ => None,
        }
    }

    /// スポイト待機中のクリック: 待機の種類に応じて筆かスロットへ拾う
    fn pick(&mut self, target: Eyedropper, pos: egui::Pos2, rect: egui::Rect) {
        match target {
            Eyedropper::Brush => self.pick_to_brush(pos, rect),
            Eyedropper::Slot => {
                self.pick_to_slot(pos, rect);
                // 「色をつくる」モーダルから来たので、拾った色を見られるよう開き直す
                self.palette_ui.color_modal = true;
            }
        }
    }

    /// 筆へのスポイト: カーソル下の湿レイヤーの顔料構成(浮遊 + 沈着、小さな窓の平均)を拾い、
    /// 筆の顔料を捨ててその比で合計1回分を取る(水は色スウォッチと同じく変えない)。パレットは変えない。
    /// 乾いた層は乾かした時点のパレットで記録されていて今の4スロットへ写せないので拾わない
    fn pick_to_brush(&mut self, pos: egui::Pos2, rect: egui::Rect) {
        let t = self.screen_to_texel(pos, rect);
        let size = self.canvas_size as f32;
        if !(0.0..size).contains(&t.x) || !(0.0..size).contains(&t.y) {
            return;
        }
        let read = {
            let renderer = self.render_state.renderer.read();
            match renderer.callback_resources.get::<GpuCanvas>() {
                Some(canvas) => canvas.read_wet_pigment(
                    &self.render_state.device,
                    &self.render_state.queue,
                    t.x as u32,
                    t.y as u32,
                    EYEDROPPER_RADIUS,
                ),
                None => Err("キャンバスが初期化されていません".to_owned()),
            }
        };
        let mut amounts = match read {
            Ok(a) => a,
            Err(e) => {
                self.status_msg = Some(e);
                return;
            }
        };
        let total: f32 = amounts.iter().sum();
        if total < EYEDROPPER_MIN_PIGMENT {
            self.status_msg = Some(
                "スポイト: ここには水彩レイヤーの絵の具がありません(乾いた層の色は拾えません)"
                    .to_owned(),
            );
            return;
        }
        // 混ざりの端に残るごく僅かな色は落とす(筆の色見本に余計な色が出ないように)
        for a in &mut amounts {
            if *a < total * EYEDROPPER_MIN_SHARE {
                *a = 0.0;
            }
        }
        self.edit_brush(|b| {
            b.clear_pigment();
            b.take_mix(amounts, paint_core::load::TAKE_DOSE);
        });
        self.after_brush_take();
        // 「色をつくる」の対象は拾った中で一番多い色にする
        let main = (0..4)
            .max_by(|&a, &b| amounts[a].total_cmp(&amounts[b]))
            .unwrap_or(0);
        self.params.brush_channel = main as u32;
        let sum: f32 = amounts.iter().sum();
        let parts: Vec<String> = (0..4)
            .filter(|&i| amounts[i] > 0.0)
            .map(|i| {
                format!(
                    "{} {:.0}%",
                    self.palette.pigments[i].name,
                    amounts[i] / sum * 100.0
                )
            })
            .collect();
        self.status_msg = Some(format!("スポイト: 筆 ← {}", parts.join(" / ")));
    }

    /// 「色をつくる」のスポイト: カーソル下1px の表示色を拾い、選択中の顔料スロット(brush_channel)へ入れる。
    /// snapshot() の読み戻し(display と同じ発色)から該当テクセルの RGB を取り出す。
    /// 表示色なので「混ざってできた見た目の色」や乾いた層の色がそのまま顔料の基本色になる
    fn pick_to_slot(&mut self, pos: egui::Pos2, rect: egui::Rect) {
        // M6: snapshot を画面と同じビュー変換(パン/ズーム/回転)で焼くので、
        // 画面 uv でそのまま索引する(= カーソル下に見えている画素)。screen_to_texel は
        // キャンバステクセルなので拡大・回転時にはズレる(zoom=1・無回転でのみ一致)
        let size = self.canvas_size;
        let su = (pos - rect.min) / rect.width().max(1.0);
        let x = ((su.x * size as f32) as i32).clamp(0, size as i32 - 1) as u32;
        let y = ((su.y * size as f32) as i32).clamp(0, size as i32 - 1) as u32;
        let data = {
            let renderer = self.render_state.renderer.read();
            match renderer.callback_resources.get::<GpuCanvas>() {
                Some(canvas) => canvas.snapshot(
                    &self.render_state.device,
                    &self.render_state.queue,
                    &self.view_uniform(),
                ),
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
        // snapshot は RGBA8 の行連続(bytes_per_row = キャンバス1辺×4、パディングなし)
        let idx = ((y * size + x) * 4) as usize;
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

    /// H6: キャンバス全体(表示モードはそのまま、ビュー変換なし)を snapshots/ に PNG 保存する。
    /// ファイル名はタイムスタンプ+プリセット名(入力欄の値。「どの設定の絵か」を残す)
    fn save_snapshot(&mut self) {
        let result = (|| -> Result<PathBuf, String> {
            let data = self.canvas_rgba()?;
            let dir = crate::assets::base_dir().join("snapshots");
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
                self.canvas_size,
                self.canvas_size,
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

    /// 解析用キャプチャの毎フレームの仕事: ツール・パラメータ・表示の変化をログへ書き、
    /// 閉じる要求が来たら一度止めて書き出し、UI スクショが届いてから(待ちきれなければそのまま)閉じる
    fn capture_frame(&mut self, ctx: &egui::Context) {
        let Some(c) = &mut self.capture else {
            return;
        };
        c.observe(
            &format!("{:?}", self.tool),
            &self.params,
            self.status_msg.as_ref(),
        );
        match c.closing {
            Closing::Running => {
                if ctx.input(|i| i.viewport().close_requested()) {
                    c.closing = Closing::WaitingShot(0);
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                        egui::UserData::default(),
                    ));
                    self.flush_capture();
                }
            }
            Closing::WaitingShot(waited) => {
                let image = ctx.input(|i| {
                    i.raw.events.iter().rev().find_map(|e| match e {
                        egui::Event::Screenshot { image, .. } => Some(image.clone()),
                        _ => None,
                    })
                });
                match image {
                    Some(image) => c.save_ui_shot(&image),
                    None if waited < capture::SHOT_TIMEOUT_FRAMES => {
                        c.closing = Closing::WaitingShot(waited + 1);
                        ctx.request_repaint();
                        return;
                    }
                    None => log::warn!("UI スクショが届かないまま閉じます"),
                }
                c.closing = Closing::Done;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Closing::Done => {}
        }
    }

    /// 解析用キャプチャの終了時の書き出し(記録・パラメータ・キャンバス PNG・作品ファイル)。2回目以降は何もしない
    fn flush_capture(&mut self) {
        if self.capture.is_none() {
            return;
        }
        let png = self.canvas_rgba();
        let work = self.export_work().and_then(|w| work::encode(&w));
        if let Some(c) = &mut self.capture {
            c.flush(&self.params, &self.palette, self.canvas_size, png, work);
        }
    }

    /// いま動いている記録(H5 の手動記録と、解析用キャプチャの記録)
    fn recorders(&mut self) -> impl Iterator<Item = &mut crate::replay::Recorder> {
        self.replay_ui
            .recorder
            .iter_mut()
            .chain(self.capture.as_mut().map(|c| &mut c.recorder))
    }

    /// キャンバス全体を表示どおりに RGBA8(行連続、canvas_size²)で読み戻す。
    /// ビュー(パン/ズーム/回転)に依らず全体を無回転で焼く
    fn canvas_rgba(&self) -> Result<Vec<u8>, String> {
        let renderer = self.render_state.renderer.read();
        let canvas = renderer
            .callback_resources
            .get::<GpuCanvas>()
            .ok_or("キャンバスが初期化されていません")?;
        canvas.snapshot(
            &self.render_state.device,
            &self.render_state.queue,
            &crate::gpu::ViewUniform::default(),
        )
    }

    /// UI スクショ(AI レビュー用): 画面全体(egui のパネル込み。キャンバスだけの
    /// H6 PNG スナップショットと違い、UI レイアウトそのものを撮る)を撮る要求を出す。
    /// egui は次フレーム以降に `Event::Screenshot` で結果を返すので、ここでは要求だけ立て、
    /// 実際の保存は `poll_ui_screenshot` が受け取ってから行う。
    fn request_ui_screenshot(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        self.screenshot_pending = true;
        self.status_msg = Some("UI スクショを撮影中…".to_owned());
    }

    /// UI スクショの結果(`Event::Screenshot`)が届いていれば固定パスへ保存する(毎フレーム呼ぶ)。
    /// AI が「最新の UI」を常に同じパスから読めるよう、タイムスタンプでなく上書き保存にする
    /// (screenshots/ui-latest.png)。UI 改善ループでこのファイルを読んで見た目を確認する。
    fn poll_ui_screenshot(&mut self, ctx: &egui::Context) {
        if !self.screenshot_pending {
            return;
        }
        // 要求は自分だけが出すので、届いた Screenshot イベントは自分の要求への返答
        let image = ctx.input(|i| {
            i.raw.events.iter().rev().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else {
            return; // まだ届いていない(撮影は数フレーム遅れる)
        };
        self.screenshot_pending = false;
        let result = (|| -> Result<PathBuf, String> {
            let dir = screenshots_dir();
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
            let path = dir.join("ui-latest.png");
            let [w, h] = image.size;
            // ColorImage::as_raw は RGBA8 の行連続(画面はスクショなので α=255)
            image::save_buffer(
                &path,
                image.as_raw(),
                w as u32,
                h as u32,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| format!("PNG の書き出しに失敗: {e}"))?;
            Ok(path)
        })();
        self.status_msg = Some(match result {
            Ok(path) => format!("UI スクショ保存(AI 用): {}", path.display()),
            Err(e) => e,
        });
    }

    /// M7: 現在の全状態(湿レイヤー・乾燥レイヤー・線画・レイヤーごとパレット・レイヤー構成・
    /// 現行パレット・SimParams)を1ファイルへ保存する。GPU から読み戻して work::save へ渡す。
    /// 描きかけ(湿った絵の具含む)から翌日続けられるようにするのが目的
    fn save_work(&mut self, name: &str) -> Result<PathBuf, String> {
        work::save(name, &self.export_work()?)
    }

    /// 現在の全状態を作品ファイルの形で GPU から読み戻す
    fn export_work(&self) -> Result<work::WorkFile, String> {
        let renderer = self.render_state.renderer.read();
        let canvas = renderer
            .callback_resources
            .get::<GpuCanvas>()
            .ok_or("キャンバスが初期化されていません")?;
        let textures = canvas.export_state(&self.render_state.device, &self.render_state.queue)?;
        let layers = canvas
            .layers
            .iter()
            .map(|l| work::StoredLayer {
                slot: l.slot,
                visible: l.visible,
                density: l.density,
            })
            .collect();
        Ok(work::WorkFile {
            // GpuCanvas の実寸を直接読む(app 側ミラー canvas_size との取り違え防止)
            canvas_size: canvas.size(),
            params: self.params,
            palette: self.palette.clone(),
            layers,
            // M5h: 乾燥レイヤーの記録時パレット(CPU 正典)も作品へ残す
            layer_palettes: canvas.layer_palettes.clone(),
            textures,
        })
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
        // M8: 保存時のサイズが現在と違えば、そのサイズでキャンバスを作り直してから復元する
        // (テクスチャ寸法が一致しないと書き戻せない。サイズは decode 済み = CANVAS_SIZES 検証済み)
        if file.canvas_size != self.canvas_size {
            self.recreate_canvas(file.canvas_size);
        }
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
                                    density: l.density,
                                })
                                .collect();
                            // M5h: 記録時パレット(decode が層数一致を検査済み=不変条件を引き継ぐ)
                            canvas.layer_palettes = file.layer_palettes.clone();
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
        // 作品のパレットは保存済みパレット(assets/palettes)とは別物なので名前追跡をリセット
        // (残すと「変更あり」表示が旧パレット名基準で誤誘導する)
        self.palette_ui.store.name.clear();
        self.line_history.clear();
        self.undo_stack.clear();
        self.stroke.end();
        self.painting = false;
        // 作品名欄は読み込んだ作品にする(続きを描いてそのまま上書き保存できる)
        self.work_ui.store.name = name.to_owned();
        // 絵が載ったので初回ガイドは出さない
        self.has_painted = true;
        self.status_msg = Some(format!("読込: {name}"));
    }

    /// 破棄すると失われる絵がキャンバスにあるか(作品読込の確認に使う)。
    /// 一度でも描いた / 作品を読み込んだ / 再生した(has_painted)か、乾燥レイヤーがあれば true
    fn canvas_has_content(&self) -> bool {
        if self.has_painted {
            return true;
        }
        let renderer = self.render_state.renderer.read();
        renderer
            .callback_resources
            .get::<GpuCanvas>()
            .is_some_and(|c| !c.layers.is_empty())
    }

    /// 左パネルのスクロール内容。セクションごとのメソッドへ振り分けるだけ。
    /// 各セクションの実装は ui サブモジュール。先頭はアクティブレイヤーのツール群
    /// (active_tools_panel が選択中レイヤーで出し分ける)。レイヤー関連は右パネル
    /// (layer_stack_panel)へ分離した
    fn tool_panel(&mut self, ui: &mut egui::Ui) {
        // 通常ユーザー向けは塗るツールだけ。作品保存・設定プリセット・PNG・新規/全消去は
        // 上部「ファイル」メニュー(file_menu)、表示(ズーム・回転)は右パネル最上段にある
        self.active_tools_panel(ui);
        // F8: 制作者向けは開発モードのときだけ露出(削除でなく退避)。
        // 味付け・診断・シミュ制御・UIスクショ / 記録再生 / シェーダー状態
        if self.dev_mode {
            self.tuning_dev_panel(ui);
            self.replay_panel(ui);
            self.shader_status(ui);
        }
    }
}

impl eframe::App for PaintApp {
    /// F11: 開発モードのトグル状態と、現在のパレット(+保存名)を永続化(次回起動時に new() が復元)。
    /// 作った色が再起動で既定4色に戻ってしまうのを防ぐ(色の管理はメイン機能)
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("dev_mode", self.dev_mode.to_string());
        if let Ok(json) = serde_json::to_string(&self.palette) {
            storage.set_string("palette", json);
        }
        storage.set_string("palette_name", self.palette_ui.store.name.clone());
        storage.set_string("auto_refill", self.auto_refill.to_string());
        if let Ok(json) = serde_json::to_string(&self.brush_refill) {
            storage.set_string("brush_load", json);
        }
    }

    /// ウィンドウの閉じる以外の経路で終わったときも、解析用キャプチャの記録は残す
    fn on_exit(&mut self) {
        self.flush_capture();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // H1: .wgsl が保存されたら再ビルド(失敗しても落とさない)
        if self.watcher.take_dirty() {
            self.rebuild_shaders();
        }
        self.poll_video_export();
        self.pull_brush();

        // M4.5d/M6: 統一 Undo/Redo(Ctrl+Z / Ctrl+Shift+Z)。線画・水彩の両方を振り分ける
        self.handle_undo_shortcuts(ui.ctx());

        // スポイト(I)・消す(E)の押しっぱなし一時切替(ペンタブレットのキー割当用)。
        // パネル描画前に self.tool / 待機フラグを確定させる
        self.handle_tool_shortcuts(ui.ctx());

        // UI スクショ(AI 用): 外部(AI の Bash)が request-shot を書いたら撮影要求を出す。
        // 続けて、撮影要求済みなら結果イベントを受け取って固定パスへ保存する
        if self.shot_watcher.take_request() {
            self.request_ui_screenshot(ui.ctx());
        }
        self.capture_frame(ui.ctx());
        self.poll_ui_screenshot(ui.ctx());

        // 上端に「ファイル」メニューバーを1本(左右・中央パネルより先に show して上端の帯を確保)
        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui));

        egui::Panel::left("tools")
            .default_size(260.0)
            .show(ui, |ui| {
                // F11: 開発モードトグルは左パネルの下端(左下)へ固定する。
                // 下端固定は他より先に確保する(残りが上側のスクロール領域になる)
                egui::Panel::bottom("dev_toggle").show(ui, |ui| self.dev_mode_toggle(ui));
                // 元に戻す/やり直しはスクロールの外に置き、常に見える位置に固定する
                self.undo_controls(ui);
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| self.tool_panel(ui));
                // F3: 操作結果(保存先・スポイト・エラー)はスクロール外の下端で常時表示する
                self.status_bar(ui);
            });

        // 右パネル: 描いている最中に手を伸ばす色まわり(乾燥・パレット)と、表示・レイヤー
        // (選択中レイヤーが左のツール群を決める)
        egui::Panel::right("layers")
            .default_size(290.0)
            .show(ui, |ui| {
                // M2: 乾燥ボタンはスクロールの外に置き、常に見える位置に固定する
                self.dry_controls(ui);
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.wet_palette_panel(ui);
                    ui.separator();
                    self.view_panel(ui);
                    self.layer_stack_panel(ui);
                });
            });

        self.error_overlay(ui);

        egui::CentralPanel::default().show(ui, |ui| self.canvas_ui(ui));

        // ファイル系モーダル(作品・新規キャンバス・全部消す)。ctx を先に clone して、
        // &mut self を捕捉する closure と ui.ctx() の同時借用を回避する(Modal は foreground Area)
        let ctx = ui.ctx().clone();
        self.file_modals(&ctx);
        // 「色をつくる」モーダル(palette.rs)
        self.color_edit_modal(&ctx);

        // 常時シミュレーションが走るため連続再描画
        ui.ctx().request_repaint();
    }
}
