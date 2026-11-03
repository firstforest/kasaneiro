# アーキテクチャ

実装の構造のリファレンス(モジュール・GPU リソース・パス順序・シェーダー・データの流れ)。**「どうなっているか」の正典**で、「なぜそうしたか」は [design.md](design.md)、要件は [requirements.md](requirements.md)、パラメータの意味は [parameters.md](parameters.md)、現在地は [status.md](status.md) を参照。

最終更新: 2026-11-01

## 1. 技術スタック

| 役割 | 選択 |
|---|---|
| GPU | wgpu(WGSL compute + render) |
| ウィンドウ / 入力 | winit(eframe 経由) |
| UI | egui / eframe / egui-wgpu(`CallbackTrait` でキャンバス描画) |
| 混色 | mixbox(依存は `crates/pigment` だけ) |
| ホットリロード | notify(ファイル監視) |
| シリアライズ | serde + serde_json(プリセット・パレット・色・ストローク記録・作品メタ) |
| 画像 | image(PNG 書き出し) |
| 動画 | 外部の ffmpeg(子プロセス。生 RGBA を stdin に流して MP4 にする。クレート依存なし) |
| 配布ビルド | include_dir(feature `embed-assets` のときだけ) |
| テスト | naga(WGSL 検証)/ egui_kittest(UI の e2e) |

依存バージョンは egui を起点にロックステップで固定する([design.md](design.md) §3.1)。Rust は mise 管理(`mise exec -- cargo run`)。

## 2. モジュール構成

workspace = CPU 純粋部の3 crate + GPU / UI を持つルートの kasaneiro crate(lib + 起動だけの bin)。分け方の理由は [design.md](design.md) §3.1。

```
kasaneiro/                (workspace ルート = kasaneiro crate(lib + bin)。[profile.*] もここ)
├─ Cargo.toml             [workspace] + [package] kasaneiro(GPU / UI / 入力)
├─ crates/
│  ├─ km/src/lib.rs       Kubelka-Munk 純関数の CPU 参照実装(依存ゼロ)
│  ├─ pigment/src/lib.rs  Palette(ランタイム4スロット)・mixbox latent / 物性 uniform(mixbox の呼び出しはここだけ)
│  └─ paint-core/         CPU 純粋部(依存は bytemuck + serde のみ)
│     └─ src/
│        ├─ sim.rs        SimParams(全パラメータの唯一の置き場)・Splat・CANVAS_SIZES
│        ├─ brush.rs      ストローク → splat 列(位置補間、筆圧の線形補間)
│        ├─ load.rs       筆の中身 BrushLoad(穂先と腹の2室。取る・浸す・洗う)・GPU 筆バッファのレイアウト BrushState・
│        │                フレームごとの筆への指示 BrushFrame(置き換え・穂先の残りの予測・補充の時間)・BrushClock
│        ├─ paper.rs      紙テクスチャの CPU 生成(紙ハイト = 値ノイズ4成分、混ざりムラの場2枚 = グラディエントノイズ)
│        ├─ replay.rs     ストローク記録・再生モデル(Recorder / Player)
│        └─ tool.rs       ツールの階層 enum(Tool = Wet / Raster、gpu_id、ToolInfo)
├─ src/                   kasaneiro crate(egui / wgpu はここだけ)
│  ├─ main.rs             eframe 起動(lib の app::PaintApp を走らせるだけ)
│  ├─ lib.rs              ライブラリ部のルート(e2e テストが同じモジュールをウィンドウなしで駆動する)
│  ├─ headless.rs         ウィンドウなしの GPU 駆動(HeadlessCanvas: 記録再生 → encode_frame → snapshot)
│  ├─ app/                egui UI
│  │  ├─ mod.rs           PaintApp の状態・ライフサイクル・ツールパネルの振り分け・App::ui / save
│  │  ├─ linehist.rs      線画の多段 Undo/Redo 履歴(RasterStroke / LineHistory / 再ラスタライズ)
│  │  └─ ui/              パネルごとに impl PaintApp を分割
│  │     ├─ mod.rs        UI 状態(PresetUi / WorkUi / PaletteUi / ReplayUi)+ 共通部品 NamedStore
│  │     ├─ tools.rs      乾燥ボタン・開発モード切替と、アクティブレイヤーごとのツールパネル
│  │     ├─ palette.rs    色ライブラリ+パレット、「色をつくる」モーダル
│  │     ├─ layers.rs     右パネルのレイヤースタック(選択・可視性・並べ替え・合成方式)
│  │     ├─ tuning.rs     調整・診断・シミュ制御(開発モード時のみ)
│  │     ├─ panels.rs     表示 / 記録再生 / シェーダー状態 / ステータスバー
│  │     ├─ file_menu.rs  メニューバー(ファイル・ヘルプ)とそのモーダル
│  │     └─ canvas.rs     キャンバス描画・初回ガイド・エラーオーバーレイ
│  ├─ gpu/                GpuCanvas
│  │  ├─ mod.rs           型定義(GpuCanvas / Pipelines / DriedLayer)・COMPUTE_SHADERS 表・
│  │  │                   clear / sync_layers / bake_dry / fast_dry / rewet / rebuild_pipelines
│  │  ├─ init.rs          GpuCanvas::new(テクスチャ・バッファ・bind group の生成)
│  │  ├─ callback.rs      CanvasCallback(egui の prepare / paint)と encode_frame(パス実行順の正典)
│  │  ├─ brush.rs         GpuBrush(塗る筆の中身の GPU バッファ・受け渡しの集計・CPU への非同期読み戻しと完了待ち)
│  │  ├─ snapshot.rs      snapshot(PNG・「色をつくる」のスポイト用の読み戻し)/ read_wet_pigment(筆へのスポイト用に
│  │  │                   湿レイヤーの浮遊+沈着を小さな窓で読み戻す)/ render_offscreen(表示の計測用)
│  │  ├─ persist.rs       export_state / import_state(作品保存の GPU 読み戻し / 書き戻し)/
│  │  │                   pingpong_mismatch(ping-pong の2枚の食い違いを数える検査用)
│  │  ├─ prelude.rs       Rust と WGSL で共有する定数と、それを WGSL の const にする shader_prelude
│  │  ├─ shader_error.rs  WGSL コンパイルエラーの行番号補正(純関数)
│  │  └─ hot_reload.rs    WGSL ファイル監視と再ビルド / UI スクショ要求の監視
│  ├─ input.rs            PointerSource trait(MouseSource / PenSource = egui Touch)
│  ├─ preset.rs           SimParams ⇄ JSON
│  ├─ replay.rs           ストローク記録の永続化(記録時パレット同梱の StoredRecording)
│  ├─ video.rs            ストローク記録の動画書き出し(HeadlessCanvas で再生 → snapshot → ffmpeg。
│  │                      VideoJob = 別スレッド実行・進捗・取り消し)
│  ├─ palette_store.rs    パレット ⇄ JSON
│  ├─ pigment_store.rs    色単体(顔料1個)⇄ JSON(保存名 = 顔料名)
│  ├─ work.rs             作品ファイル works/*.kasane の encode / decode
│  └─ assets.rs           基準ディレクトリ解決(base_dir)・配布ビルドの既定 JSON 書き出し・
│                         保存名の正規化・アトミック書き込み・JSON ストア共通関数
├─ tests/
│  ├─ shader_compile.rs     全シェーダーを全キャンバスサイズのプレリュード付きで naga 検証
│  ├─ sim_params_layout.rs  SimParams の Rust 定義と common.wgsl の struct の一致検査
│  ├─ e2e_watercolor.rs     headless でストローク再生 → 3要件を色の指標で判定
│  ├─ e2e_video.rs          記録を MP4 に書き出す・取り消しで書きかけを消す(ffmpeg が無ければ飛ばす)
│  ├─ e2e_ui.rs             egui_kittest で PaintApp を実入力で操作 → 画面と UI 状態を判定
│  ├─ perf_bench.rs         GPU の所要時間の計測(#[ignore])
│  └─ common/mod.rs         e2e の共通部品(テスト用 wgpu・記録の組み立て・色の指標)
└─ assets/
   ├─ shaders/*.wgsl      実行時ロード(配布ビルドのみ埋め込み)
   ├─ presets/*.json      SimParams プリセット(git 管理)
   ├─ strokes/*.json      テストストローク(git 管理。記録時パレット同梱)
   ├─ palettes/*.json     パレット = 4色一式(git 管理。default は default_palette() と同値)
   ├─ pigments/*.json     色単体ライブラリ(顔料1個 = 1ファイル。git 管理)
   ├─ fonts/              UI フォント M PLUS 1p(OFL。include_bytes で埋め込み)
   └─ licenses/third-party.txt  依存クレートのライセンス一覧(include_str で埋め込み、
                          「かさねいろについて」が表示する。再生成コマンドはファイル冒頭)
```

- `replay` は**モデル**(Recorder / Player / Recording)が paint-core、**永続化**が kasaneiro crate
- パスの基準は `assets::base_dir()` の1か所: 通常ビルドは `CARGO_MANIFEST_DIR`(どこから起動しても効く)、配布ビルドは exe のあるディレクトリ。assets/ works/ snapshots/ videos/ screenshots/ はすべてこの直下。works/ snapshots/ videos/ screenshots/ は git 管理外

## 3. GPU リソース

### 3.1 キャンバスサイズ

- 選択肢は `CANVAS_SIZES = [512, 1024, 2048]`(正方形のみ)、既定 `DEFAULT_CANVAS_SIZE = 512`。実行時の値は `GpuCanvas.size` が正典で、全テクスチャ・dispatch・読み戻しの寸法がここから決まる
- サイズ変更 = `PaintApp::recreate_canvas` で GpuCanvas を作り直す(旧インスタンスは callback_resources の差し替えで drop)。UI はファイルメニューの「新規キャンバス…」
- 64 の倍数なので読み戻しの行バイト数は常に 256B 整列(パディング不要)
- VRAM は 2048² で計 ~1.3GB(乾燥レイヤー8枚の 512MB が支配的。下地は 64MB。窮屈なら `MAX_LAYERS` を減らす)
- テクスチャ・バッファは作成時に wgpu がゼロ初期化する。「全部消す」(`clear`)と線画 Undo(`clear_line`)のゼロ化は、`Features::CLEAR_TEXTURE`(アダプタ対応時だけ要求)があれば `clear_texture`、なければゼロ埋めの `write_texture`

### 3.2 テクスチャ

| テクスチャ | 形式 | 内容 |
|---|---|---|
| 水 | rgba32float × ping-pong 2枚 | r=水量 / g=速度x / b=速度y / a=濡れマスク |
| 浮遊顔料 | rgba32float × 2 | rgba 各チャンネル = 顔料1種(パレットのスロットと 1:1)。水の流れで移流する |
| 沈着顔料 | rgba32float × 2 | 同上。紙に定着した分で移流しない |
| 紙 | rgba32float × 1(静的) | r = 紙ハイト 0..1(0=谷 / 1=山)、g, b = 混ざりムラの場2枚 0..1、a = 未使用。起動時に CPU 生成 |
| 乾燥レイヤー | rgba32float texture array(最大8スライス) | 1スライス = 1乾燥レイヤー、rgba = 4顔料濃度 |
| 乾燥レイヤーの合成キャッシュ | rgba16float × 1 | 可視な乾燥レイヤーを合成した結果(compose_mode=0 は透過率の積、1 は紙+乾燥層の KM 反射率(リニア))。§5 |
| 下地 | rgba16float texture array(2スライス) | 下の乾燥レイヤーを焼き固めた合成結果(0 = 乗算の透過率の積 / 1 = KM 反射率)。`has_base` の間だけ合成の出発点になる。§6 |
| 線画 | r32float × 3(鉛筆・ペン・ハイライト) | r = インク濃度 / 不透明度 0..1。ping-pong せず read_write で自己更新。§7 |
| 湿レイヤー退避 | rgba32float × 3(水・浮遊・沈着、COPY のみ) | 水彩ストローク開始時に `current` の3枚を GPU 間コピーで退避し、水彩の undo で書き戻す。最新1本ぶん |

- **ping-pong は水・浮遊・沈着の3テクスチャまとめて単一の `current`**。各 compute パスは3枚の src を読み、3枚の dst を書いて(変更しない分は素通し)反転する。書くのは計算するタイルと2枚が食い違いうるタイルだけ(§8)。線画テクスチャは `current` に依存しない
- 乾燥レイヤー array・線画3枚・下地・顔料 latent uniform には作品保存のために `COPY_SRC` / `COPY_DST` を付ける(乾燥レイヤー array はスロットの詰め直し、合成キャッシュは下地への書き写しにも使う)。`copy_texture_to_buffer` に実体が要るので、乾燥レイヤーはスライスビューとは別に実体テクスチャを `dried_texture` として持つ

### 3.3 バッファ

| バッファ | 内容 |
|---|---|
| SimParams uniform | 毎フレーム書く(§11) |
| splat storage | ヘッダ(splat 数・流体へ流す数・腹から穂先へ補充する時間 `brush_dt`)+ splat 列。上限 `MAX_SPLATS=8192` |
| 筆の中身 storage | 塗る筆の中身 `Brush`(穂先と腹それぞれ水 + 4色の顔料、64B。read_write)。正典は GPU。CPU は `set` で置き換え、中身が変わったフレームに MAP_READ のバッファへコピーして非同期で読み戻す(§9) |
| 受け渡しの集計 storage | `array<atomic<i32>, 5>`(水 + 4色の顔料、穂先へ入った向きが正、固定小数点 × `FLOW_SCALE`)。splat が足し、brush が穂先へ移して 0 に戻す |
| 顔料個性 uniform | ρ/ω/γ/μ(全レイヤー共通) |
| 顔料 latent uniform | `array<vec4f, LATENT_TOTAL>`(§3.5) |
| LayerUniform | 乾燥レイヤーの重ね順 + visible_mask + 層ごとの濃度倍率(density。重ね順で並べる)+ has_base(下地から合成するか)+ force_mode(0 = compose_mode に従う / 1 = 乗算 / 2 = KM。下地への統合用) |
| ViewUniform | パン / ズーム / 回転(§3.6) |
| アクティブタイル | `raw_active` / `active` / `tile_dirty`(いずれも `array<u32>`、要素数 = (size/16)²)。§8 |

### 3.4 binding レイアウト

compute は種別ごとに5レイアウト(`ComputeLayout`)+ dried_cache:

- **共通**(splat と流体パス。[common.wgsl](../assets/shaders/common.wgsl)): `0/1` 水 src/dst、`2/3` 浮遊 src/dst、`4/5` 沈着 src/dst、`6` SimParams、`7` splat storage、`8` 紙(r = 紙ハイト、g, b = 混ざりムラの場)、`9` 顔料個性、`10` 清書ペンの線画(透水率。読むのは velocity / diffuse だけ)、`11` アクティブタイルの状態(splat / velocity / relax / flowout / advect / diffuse / transfer が読む。fastdry / rewet は宣言せず全面計算)、`12` 筆の中身、`13` 吸い込みの集計(splat / brush だけが宣言する)
- **bake**: 共通の `0..6, 8` + `9` = 乾燥レイヤーの書き込みスライス(splat なし)
- **raster**(linesplat.wgsl): `0` 対象の線画テクスチャ(read_write r32float)、`1` SimParams、`2` splat storage、`3` 紙ハイト。描画先は bind group の差し替えで選ぶ(パイプラインは1本)
- **tilescan**: `0` 水・`2` 浮遊顔料(read。parity 別 bind group)、`3` SimParams、`4` splat storage、`5` raw_active(write)
- **tiledilate**: `0` raw_active(read)、`1` active(write)、`2` SimParams(common.wgsl が参照するため束ねるだけ)、`3` tile_dirty(read_write)
- **dried_cache**(display.wgsl の `dried_cache_main`): `3` SimParams、`4` 顔料 latent、`6` 乾燥レイヤー array、`7` LayerUniform、`14` 合成キャッシュ(write-only storage、rgba16float)、`15` 下地(2d array)

display: `0/1/2` 水 / 浮遊 / 沈着、`3` SimParams、`4` 顔料 latent、`5` 紙ハイト、`8/9` 線画(鉛筆 / ペン)、`10` ハイライト、`11` ViewUniform、`12` アクティブタイルの状態(デバッグ表示モード7)、`13` 乾燥レイヤーの合成キャッシュ。

### 3.5 顔料 latent のレイアウト

`array<vec4f, LATENT_TOTAL=78>`:

- 先頭 `GLOBAL_LATENTS=6` vec4 = パレット非依存のグローバル光学(`[0,1]` 紙 / `[2,3]` 白 R_w / `[4,5]` 黒 R_b)
- 続いて**パレット枠**を `PALETTE_SLOTS = MAX_LAYERS+1 = 9` 個。各枠 `PIGMENT_LATENTS=8` vec4(顔料4種 × c0..c3 / RGB 残差)。枠 `0..MAX_LAYERS-1` は乾燥レイヤーのスロット別、枠 `LIVE_PALETTE=MAX_LAYERS` は湿レイヤーの現行パレット
- 乾かすと現行パレットの色を対応スロットの枠へ焼き込む(`GpuCanvas::record_layer_palette`)。同時に `bake_dry(…, palette)` が `GpuCanvas::layer_palettes`(index = slot)へ Palette を丸ごと clone する。名前・物性込みの正典はこちらで(不変条件 `len == layers.len()`、使用中のスロットは常に 0..len)、乾燥層パネルの「パレット抽出」が読む。下地への統合で残った層を詰め直すときは、この正典から各スロットの枠を書き直す
- 顔料の編集時は `set_palette` が顔料個性 uniform と live 枠だけを `write_buffer`(パイプライン再構築不要)
- 定数は [crates/pigment](../crates/pigment/src/lib.rs) と [src/gpu/prelude.rs](../src/gpu/prelude.rs) が正典で、WGSL へは `shader_prelude` が const として渡す。WGSL 側の索引は `pal_base()`

### 3.6 ビューポート変換(ViewUniform)

- `ViewUniform { center: vec2f, span: f32, cos_t: f32, sin_t: f32, _pad×3 }`(32B。末尾を vec3f にすると align 16 で 48B に膨らむので f32×3 で詰める)。`CanvasCallback` がフレームごとに書く
- fs_main が画面 uv → キャンバス uv を `canvas_uv = center + R(θ)·(画面uv − 0.5)·span`(`span = 1/zoom`)で写してからサンプルするので、拡大 / パン / 回転が全サンプルに一括で効く。キャンバス外は `BG_COLOR`(紙の周りの机)
- `zoom ∈ [1, 32]`。クランプは回転なしなら窓をキャンバス内に(`center` を各軸 `[half, 1−half]`)、回転時は `center` だけ `[0,1]` に留める
- ポインタ → テクセル変換(描画・スポイト)も同じ写像 `PaintApp::screen_to_texel`(逆回転込み)を通す
- 「色をつくる」のスポイト(表示色)の読み戻しは画面と同じビューで焼いて画面 uv で索引する。筆へのスポイトは `screen_to_texel` のテクセルを中心に湿レイヤーの浮遊+沈着テクスチャを直接読む。PNG 書き出しは `ViewUniform::default()`(全体・無回転)で焼く
- 操作: 回転は Shift+ホイールで15°刻み・「ビュー」パネルのスライダーで自由角。パンは中ボタンまたはスペース+左ドラッグ

## 4. フレームの流れ

egui-wgpu の `CallbackTrait`(`gpu/callback.rs` の `CanvasCallback`)で駆動する。compute は `GpuCanvas::encode_frame` に集約してあり、prepare と headless 駆動(e2e)が同じ経路を通る:

```
app(毎フレーム): SimClock がフレームの経過時間を 1/60 秒の刻み数(最大4)に変える。
  記録・再生は刻みごとに進め、sim_steps = 刻み数 × 速度倍率(一時停止中は 0、1ステップなら 1)
prepare(毎フレーム)= encode_frame:
  (prepare の頭で、前のフレームまでに積んだ筆の中身の読み戻しを進める)
  SimParams を uniform に write_buffer(スライダー即時反映)
  筆の中身の置き換え(再生のストローク開始)があれば筆バッファへ write_buffer
    (アプリの色・水・洗う・自動補充はフレーム外でその場で write_buffer)
  splat storage へヘッダ+splat 列を書く(0 個でもヘッダは書く。一時停止中でもブラシは反映。
    MAX_SPLATS 超過分は警告ログを出して切り捨て。fluid_count は線画ツールのフレームで 0)
  ラスタツール(line_target=Some)→ 専用 line_pass で linesplat.wgsl(線画テクスチャへ直描き)
  乾燥レイヤーの合成キャッシュ: dirty か pigment_density / compose_mode が変わったフレームだけ dried_cache_main
  (ping-pong するパスが1つもないフレーム = 一時停止中で流体へ筆を流さない、はここで終わり)
  sim_pass:
    tilescan → tiledilate(アクティブタイルの決定。§8)
    splat.wgsl(流体ツール: 水+初速+顔料の注入。tool で分岐。塗る筆は穂先と紙の受け渡しを集計へ足す)
    brush.wgsl(塗るツールで splat があるフレームだけ。1スレッドで、受け渡しを穂先へ → 腹から穂先へ補充)
    sim_steps 回:
      velocity → relax × relax_iters
      → flowout(edge_eta > 0 のときだけ) → advect(水+浮遊顔料)
      → diffuse × diffuse_iters(顔料+水の毛細管拡散)→ transfer(吸着 / 脱着+蒸発)
  筆の中身が読み戻した値より新しく、読み戻しバッファが空いていればコピーを積む
paint:
  display.wgsl でフルスクリーン三角形を描画(合成キャッシュの上に湿レイヤーを発色して重ね、線画・デバッグ表示)
```

- 線画のライブ描画を流体と別の compute パスにするのは、流体パスがペン線を sampled で読むため(同一パス内で read_write と sampled を兼ねると使用範囲が衝突する)
- ボタン駆動の単発パス: **bake**(乾かす = 焼き込み)/ **fastdry**(水だけ除去)/ **rewet**(全面再湿潤)。これらと水彩の undo・作品読込・全部消すの後は `GpuCanvas::mark_tiles_dirty` で全タイルを dirty にする(§8)
- **PNG スナップショット**は display と同じシェーダーでオフスクリーンに焼いて読み戻す
- **UI スクショ**(AI レビュー用)は egui 経路: `ViewportCommand::Screenshot` で要求し、返ってきた `Event::Screenshot` を `poll_ui_screenshot`([app/mod.rs](../src/app/mod.rs))が固定パス `screenshots/ui-latest.png` へ上書き保存する。トリガーは UI ボタンと、`screenshots/request-shot` の作成 / 変更を拾う `ScreenshotWatcher`([gpu/hot_reload.rs](../src/gpu/hot_reload.rs))。自身が書く `ui-latest.png` はファイル名で除外して撮影ループを防ぐ

### シェーダー一覧(assets/shaders/)

| ファイル | 役割 |
|---|---|
| common.wgsl | 共通定義(SimParams / Splat / Brush / BrushIntake 構造体、濡れ判定、バイリニア補間、`tile_index_of`)。各シェーダーの先頭に連結する(§11) |
| splat.wgsl | ブラシ入力。tool(描画 / リフト / 消去 / 吸い取り)で分岐。描画は穂先と紙が濡れ具合の差で水を受け渡す(穂先の濡れ具合 = 穂先の水 × `Splat.scale`(置く時点の残りの予測)、紙の濡れ具合 = 水位 ÷ (`brush_water` × `SAT_RATIO`))。出るときは穂先の濃さ、入るときは紙の浮遊顔料の濃さで顔料も動き、受け渡した量を集計へ atomic で足す。出た水は筆の速度(`brush_velocity` × 穂先の濡れ具合 × 進行方向)を持ち、紙の水と運動量で混ぜる。置いたままの splat(`feed`)は初速なし・溶かし戻し弱め |
| brush.wgsl | 塗る筆の中身の更新(1スレッド)。受け渡しの集計を穂先の容量で割って穂先に足し引き → 腹が穂先より濡れていれば `brush_dt` / `brush_refill_time` で腹から穂先へ補充(一方向、顔料は腹の濃さ)。室の容量は「満タンの室だけで乾いた紙をこの長さ塗ると半分」の長さ(`brush_tip` / `brush_hold`)から。集計は 0 に戻す |
| velocity.wgsl | 速度更新(水面勾配 = 水深 + paper_amp×紙ハイト → 加速)。ペン線の透水率を速度場・にじみ拡張に掛ける。にじみ拡張は乾いたセルの沈着顔料の量でも変える(白い紙は `wick_bare` 倍) |
| relax.wgsl | 発散の反復緩和(δ = −ξ·div。濡れセルのみ、乾いたセルは壁) |
| flowout.wgsl | FlowOutward(縁の水を抜く)。既定オフ(edge_eta 0 で dispatch ごと省略) |
| advect.wgsl | セミラグランジアン移流(水量・速度・浮遊顔料)に MacCormack 補正を `advect_sharp` の割合で掛ける。補正に要る逆向きの値は、戻り先を囲む4格子点でその場で前進移流して求める(1パス・追加テクスチャなし) |
| diffuse.wgsl | 浮遊顔料+水の拡散(陽解法・濡れセル間のみ・保存則あり)。顔料は粒の細かさ μ で per-channel に拡散倍率が変わり、溜まりやすさ φ(紙の谷への寄り `paper_drift`×γ + 混ざりムラ `mix_mottle`)の差で流束が偏る。水は毛細管拡散([note/05](note/05-wet-spreading.md))。ペン線を挟む流束に透水率を掛ける |
| transfer.wgsl | 吸着 / 脱着(ρ/ω で per-channel 変調)+蒸発 |
| bake.wgsl | 乾かす = 乾燥レイヤーへの焼き込み+湿レイヤー全ゼロ |
| fastdry.wgsl | 浮遊 → 沈着に落として水と流れをゼロに(焼き込まない) |
| rewet.wgsl | 全面マスク=1+rewet_water(沈着は既存の脱着で再浮遊) |
| linesplat.wgsl | ラスタ線画。鉛筆 / ペン / ハイライトを対象テクスチャへ直描き(line_mode で見た目を分岐、line_eraser で減算) |
| tilescan.wgsl | アクティブタイル第1段。タイルごとに濡れとブラシ近傍を走査して raw_active を書く |
| tiledilate.wgsl | アクティブタイル第2段。raw_active を3×3で膨張し、dirty と合わせて active を確定 |
| display.wgsl | 表示: 層内発色 → レイヤー合成 → 線画合成 → sRGB。デバッグ表示の分岐もここ。乾燥レイヤーの合成は compute 入口 `dried_cache_main` がキャッシュへ焼く(色の関数を共有するため同じファイル) |

## 5. 発色と合成

方針(層内 mixbox / 層間 KM)の理由は [design.md](design.md) §3.4。

- **CPU 側**([crates/pigment/src/lib.rs](../crates/pigment/src/lib.rs)): グローバル光学(紙+白+黒)と `Palette` の顔料 latent を計算して uniform で渡す。パレットは起動時に eframe storage から復元し(無い・壊れていれば既定パレット)、UI で編集するたびに再計算する
- **層内**(display.wgsl): 画素ごとに4顔料の濃度比で latent を線形混合 → latent→RGB 多項式(mixbox eval_polynomial の WGSL 移植)。紙とは被覆率 `1−exp(−pigment_density·総濃度)` で混合
- **層間**: 紙(下地があれば下地)→ 乾燥レイヤー(LayerUniform の order 順)→ 湿レイヤーを、白地・黒地の発色から出した R, T でリニア色空間で下から合成(`compose_mode=1`)。`compose_mode=0` は multiply
- **キャッシュ**: 乾燥レイヤーまでの合成は `dried_cache_main` が rgba16float に焼き、fs_main は湿レイヤーだけを毎フレーム発色して重ねる。焼き直すのは乾燥レイヤーの中身・並び・可視性・濃度倍率・記録パレット・シェーダー(`dried_cache_dirty`)か `pigment_density` / `compose_mode` が変わったフレームだけ。フレームを挟まない snapshot は dirty なら自分で焼く

## 6. 乾燥とレイヤー

- 乾かす = bake パスが `(浮遊+沈着) × dry_shift × 粒状感ゲート × (1 + dry_edge×縁バンド)` を texture array の新スライスへ書き、湿レイヤーを全ゼロにする
- レイヤーの重ね順・可視性・濃度倍率は `LayerUniform`(order + visible_mask + density)で `dried_cache_main` へ渡す。倍率は読んだ顔料濃度に掛けてから発色・合成する。UI の並べ替えがそのまま合成順になる。`sync_layers` が合成キャッシュの焼き直しを予約する
- **下地への統合**(`merge_into_base`): 下から N 枚だけを並べた LayerUniform を `force_mode` 1 / 2 で2回 `dried_cache_main` に通し、それぞれの合成キャッシュを下地のスライス 0 / 1 へコピーして `has_base` を立てる。続けて残りの層を重ね順のままスロット 0.. へ詰め直す(同じテクスチャ内のスライス間コピー。行き先にまだ動かしていない層がいれば空きスロットへ逃がす `compaction_moves`)。`clear` で `has_base` を下ろす
- 水彩の undo: ストローク開始時に `current` の3枚を退避テクスチャへコピーし、Ctrl+Z で書き戻す

## 7. 線画

下書き鉛筆・清書ペン・白ハイライトのラスタツール(`Tool::Raster`)。

- **描画経路**: 選択中ツールが `Tool::Raster` のとき `PaintApp::line_target()` が `Some(LineTarget)` を返し、`CanvasCallback` がブラシ入力を `linesplat.wgsl` へ回す。`line_mode` が見た目を分岐(鉛筆 = 柔エッジ・紙目粒状・筆圧→濃さ / ペン = 硬エッジ・筆圧→太さ / ハイライト = 硬めエッジの不透明白・筆圧→不透明度)
- **蓄積**: 目標濃度への `max`(1フレーム内で密にサンプルしても一定の濃度へ収束)
- **合成**: display.wgsl で色を合成した後、`apply_lines()` が鉛筆(グレー)→ ペン(濃色)→ ハイライト(白)の順にアルファ合成。`show_pencil` / `show_pen` / `show_highlight` で非表示にできる
- **透水率**: 清書ペン濃度から `perm = 1 − line_block×ペン濃度` を出し、velocity(速度場・にじみ拡張)と diffuse(隣接流束)に掛ける。`line_block=0` で境界なし
- **多段 Undo/Redo**([src/app/linehist.rs](../src/app/linehist.rs)): ストローク単位で生ポインタ点+実効 SimParams を `LineHistory` に持つ。Undo = 対象テクスチャを `clear_line` してから残りを `rasterize_line` で引き直す / Redo = 取り消し分を再適用(Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y)。長い線は MAX_SPLATS 単位に分けて dispatch
- **記録との関係**: ストローク記録は流体ツールだけが対象(`line_target` が Some の間は recorder をスキップ)

## 8. アクティブタイル

計算範囲を濡れ面積に絞る仕組み。方式の理由は [design.md](design.md) §3.7。

- **判定**: tilescan がタイル(`TILE_SIZE`=16×16)ごとに、動きうる濡れたセル(濡れマスク a > 0.5 で、水・速度・浮遊顔料(> `SUSP_EPS`=1e-6)のどれかが残る)があるかと、このフレームの流体 splat の近傍かを見て `raw_active` を作る。顔料が載っているだけで乾いたタイルや、蒸発で水が尽き浮遊顔料も沈み切ったタイルは立てない(濡れマスクは残るが、水・速度・浮遊が 0 のセルは全パスで不動点。隣から水が入るセルは水の残るセルのタイル+膨張に含まれる)
- **膨張**: tiledilate が 3×3 で膨張して `active` を確定する。1タイル分の余裕は「濡れ前線が1フレームに進む距離 < TILE_SIZE」のための halo(既定 vel_max×sim_steps ≪ 16px)
- **3状態**: `TILE_ACTIVE`(計算)/ `TILE_COPY`(計算しないが dirty → 3テクスチャを src→dst に素通しコピー)/ `TILE_COLD`(何も書かずに return)。定数は gpu/prelude.rs が正典
- **dirty**: tiledilate が `dirty = 今フレーム有効か` を残し、次にシミュが進むフレームで有効でなければ COPY として2枚を揃える。この更新は「そのフレームに少なくとも1パス進む」前提なので、ping-pong するパスが1つもないフレームは tilescan / tiledilate ごと積まない。`current` 側だけを書き替えるフレーム外の操作の後は `mark_tiles_dirty` で全タイルを dirty にする
- **ハザード**: tiledilate が書いた `active` を後続パスが読む storage RAW は wgpu の自動バリアで解決する
- **退避と可視化**: `active_tiles=0` で全タイル有効(全面計算)。デバッグ表示モード7が状態を可視化(計算中 = 緑 / コピーだけ = 青 / 何もしない = 暗く + タイル格子)
- **検査**: e2e の `active_tiles_match_full_compute`(全面計算と同じ絵)、`active_tiles_match_full_compute_after_evaporation`(水が尽きた跡の隣に水を置いても同じ絵)と `inactive_tiles_keep_pingpong_in_sync`(2枚を読み戻して比較)

## 9. 入力の経路

```
ペン → Windows Ink(WM_POINTER)→ winit(GetPointerPenInfo で筆圧)→ egui::Event::Touch{force}
  → input.rs PenSource → PointerEvent(論理ピクセル+筆圧 0..1)
  → paint_core::brush(ストローク補間。サンプル間隔は筆圧反映後の実効半径から SimParams::splat_spacing)
  → 塗る筆なら paint_core::load::BrushFrame::drag(各 splat に置く時点の穂先の残りの予測を載せる。乾いた紙に置いた場合 = 最も減る場合)
  → Splat 列 + BrushFrame → splat storage buffer → splat.wgsl(穂先と紙の受け渡しを集計)→ brush.wgsl
```

- `PointerSource` trait で Mouse / Pen を抽象化する。ペン接地中はマウス入力を無視する(egui-winit が Touch からポインタをエミュレートするため二重ストロークになる)
- 筆圧マッピングは `実効値 = 基準値 × mix(1, 筆圧^γ, 効き)` を半径・水量・顔料量に適用(splat.wgsl と CPU 側で同式)
- **筆の中身**: 正典は GPU の筆バッファ(`GpuBrush`)。`PaintApp` は `brush_load`(フレーム頭に読み戻した値。描いている間は数フレーム遅れる)と `brush_refill`(自動補充の戻り先 = 最後に色・水・洗うで作った中身)を持つ。色・水・洗う・自動補充・再生の停止は、`sync_brush`(GPU の完了を待って最新の値を得る。読み戻しが追いついていれば待たない)→ CPU で `BrushLoad` を変える → `set_brush` で筆バッファへその場で write_buffer、の順で置き換える。置き換えをフレームのコールバックに載せないのは、egui が UI を組み直したパスのコールバックを捨てるため。ツールバーの色・水スウォッチは押した瞬間と押し続けている間の2段で中身を変える。`brush_refill` と自動補充の on/off は eframe storage に保存する
- **受け渡しと補充**: CPU はフレームごとに `BrushFrame`(穂先の残りの予測 `scale` と補充の時間 `dt`)を `CanvasCallback.brush` で渡すだけで、筆の中身そのものは GPU で進む。補充の時間は `BrushClock` が刻みを溜め、brush パスが走るフレーム(塗るツールで流体へ splat を流すフレーム)でまとめて渡す(上限 2 秒)。記録の再生は `Player` がストロークの始めに `BrushFrame.set` で記録した中身へ置き換えるので、受け渡しまで含めて画面と headless で同じ経路を通る
- **置いたまま**: 塗る筆を置いたまま動かないフレームは、読み戻した穂先か腹に水が残る間、`PaintApp::hold_still` が毎フレーム置いたままの splat(`Splat.feed=1`)を1つ積む。受け渡しの式は同じなので、紙と釣り合うまで色水が出続け、毛細管拡散が外へ運ぶ。ストローク記録は生ポインタ点のみなので置いたままの splat は再生で再現されない
- ストローク記録は補間前の生ポインタ入力と、ストロークごとに筆を下ろした時点の筆の中身(`RecordedStroke.load`)を保存する。`load` の無い記録はそのストロークの顔料スロットを標準の1回分含んだ筆で再生する。時刻は 1/60 秒の刻み番号(JSON のキー名は `frame`)

## 10. 保存

- **JSON ストア**(プリセット・パレット・色・ストローク): `assets.rs` の `json_save` / `json_load` / `json_delete` / `json_list` を共通に使う。保存名は `sanitize_name` で正規化し、書き込みは `.tmp` → rename でアトミックにする
- **作品ファイル**(`works/*.kasane`、[src/work.rs](../src/work.rs)): 先頭に `MAGIC "KSN1"` + メタ長 + メタ JSON(SimParams・現行パレット・レイヤー構成 `[slot, visible, density]`・レイヤーごとパレット・canvas_size・layer_count・has_base)、続けて生ブロブを固定順(f32 の湿レイヤー3 → 乾燥レイヤー → 線画3 → 顔料 latent → `has_base` のときだけ下地2スライスの f16 生ビット)で並べる。`has_base` は省略時 false、`density` は省略時 1
  - decode はメタの layer_palettes と層数の一致・レイヤー構成・末尾の余分なバイトを検査する(`layer_palettes` の不変条件の入口)。現行形式だけを読む
  - 読込時に canvas_size(`CANVAS_SIZES` で検証)が現在と違えば、そのサイズでキャンバスを作り直してから復元する
  - GPU ⇄ f32 配列は `export_state` / `import_state`([src/gpu/persist.rs](../src/gpu/persist.rs))。読み戻しは全テクスチャのコピーを1回の submit にまとめる。parity は保存せず、読込時に `current=0` へ正規化する
  - 線画の Undo 履歴は保存しない(読込直後の Undo と水彩の undo は効かない)
- **リプレイ動画**(`videos/*.mp4`、[src/video.rs](../src/video.rs)): 画面と同じ Device / Queue で別の `HeadlessCanvas` を作り、別スレッドで `Player` → `encode_frame` → `snapshot` を1フレームずつ回す。読み戻した RGBA8(size²)を ffmpeg(`-f rawvideo -pix_fmt rgba` → libx264 / yuv420p / crf 18)の stdin へ書く。再生が終わってから `TAIL_FRAMES`(2 秒)だけ入力なしで撮り続ける
  - ffmpeg の場所: 環境変数 `KASANEIRO_FFMPEG` → `base_dir()` 直下の ffmpeg(.exe) → PATH。Windows では `CREATE_NO_WINDOW` で起動する
  - UI は `VideoJob` の進捗(書き終えたフレーム数 / 再生 + 余韻の予定数)を出す。取り消しは次のフレームの前で打ち切り、ffmpeg を止めて書きかけを消す。完了は `ui()` が毎フレーム `poll_video_export` で拾う(同時に1本)

## 11. ホットリロードとパラメータ反映

- WGSL は `assets/shaders/` から実行時ロード。notify で監視し、保存で `rebuild_pipelines()`。コンパイルエラー時は描画をスキップしてエラーオーバーレイを出し、直前の正常なパイプラインで続行する
- **シェーダーの連結は3段**: `shader_prelude`(Rust と共有する const: TILE_SIZE / TILES_PER_SIDE / MAX_LAYERS / LAYER_ORDER_VEC4S / GLOBAL_LATENTS / PIGMENT_LATENTS / LIVE_PALETTE / LATENT_TOTAL)+ common.wgsl + 本体。エラーの行番号は連結分ずれるので `remap_shader_error_lines` で補正して表示する
- compute パイプラインは `COMPUTE_SHADERS` の表(WGSL ファイル名 → レイアウト種別)から作る。参照は名前引き `Pipelines::compute("splat.wgsl")`、実行順は `encode_frame()` のハードコード
- `SimParams` は毎フレーム uniform へ書く(スライダー即時反映)。WGSL の uniform 規則(16 バイト整列)に合わせ、現在は 62 フィールド+末尾パディング2 = 256B。足すときの手順は [parameters.md](parameters.md) §11
- **配布ビルド**(`cargo build --release --features embed-assets`):
  1. `assets/shaders/` を `include_dir!` で埋め込む(取得元の分岐は `GpuCanvas::shader_source` の1か所。ファイル監視は無効)
  2. 既定 JSON(pigments / palettes / presets / strokes)も埋め込み、初回起動時に `assets::seed_default_assets()` が exe 隣へ書き出す(ディレクトリ単位で「無ければ書く」— ユーザーの編集・削除を復活させない)
  3. パス基準が exe のディレクトリになる

## 12. テスト

`cargo test --workspace` で全テスト(e2e は GPU 必須)。件数は [status.md](status.md) §1.4。検証の分担の考え方は [design.md](design.md) §2。

| 対象 | 内容 |
|---|---|
| km crate | KM 純関数(簡約 vs 一般式の一致・K,S 往復・透明 / 不透明 / グレーズ・K=0 で NaN にならない) |
| pigment crate | 黄+青=緑、latent 往復、物性の範囲とクランプ、古い JSON の既定値 |
| paint-core crate | SimParams 整列・筆圧式、brush 補間、replay 往復、tool の gpu_id 往復、paper 生成 |
| kasaneiro crate | 保存名の正規化・JSON ストア往復、シェーダーエラー行番号補正、同梱アセットの読込とコード既定値の一致、作品ファイルの往復と破損の拒否 |
| [tests/shader_compile.rs](../tests/shader_compile.rs) | 全シェーダーを全キャンバスサイズのプレリュード付きで naga 検証 |
| [tests/sim_params_layout.rs](../tests/sim_params_layout.rs) | SimParams の Rust 定義と common.wgsl の struct のフィールド名・順序・型の一致 |
| [tests/e2e_watercolor.rs](../tests/e2e_watercolor.rs) | `HeadlessCanvas` でストロークを再生し(アプリの再生と同じ `Player` → `encode_frame` 経路)、wet-on-wet / グレージング / 消去 / 削り / ステイニング差 / 同梱ストロークの再生 / アクティブタイルの不変条件を判定 |
| [tests/e2e_video.rs](../tests/e2e_video.rs) | 記録を MP4 に書き出し、フレーム数と最後のフレームに絵があることを確かめる / 取り消しで書きかけが残らない。ffmpeg が起動できなければ何もせず通す |
| [tests/e2e_ui.rs](../tests/e2e_ui.rs) | egui_kittest で PaintApp 全体を起動し、マウス・ペン(筆圧付き Touch)・ツールボタン・モーダル・乾かすを実入力で送って描画結果と UI 状態を判定 |
| [tests/perf_bench.rs](../tests/perf_bench.rs)(ignore) | 512 / 1024 / 2048 で乾いた紙・一部濡れ・全面計算のフレーム時間と、乾燥レイヤー 0 / 8 枚の表示時間を表で出す。`cargo test --test perf_bench -- --ignored --nocapture` |

- e2e の結果画像は `target/tmp/e2e/*.png`。落ちたらまず画像を見て、体験として正しければ閾値側を直す
- UI 要素は AccessKit の名前(キャンバス・「塗る色 N: 顔料名」・ボタン文字)で引く。キャンバスのシミュは prepare で進むので毎フレーム `render()` する。step_dt は実機と同じ 1/60 秒
- テスト用 wgpu デバイスは egui_kittest の `create_render_state`(ウィンドウなし)で、アダプタは実 GPU(ディスクリート → 内蔵の順)を選ぶ([tests/common/mod.rs](../tests/common/mod.rs) の `render_state`)。`KASANEIRO_TEST_ADAPTER=cpu` で egui_kittest 既定のソフトウェアラスタライザ優先(Windows では DX12 WARP)に戻る
