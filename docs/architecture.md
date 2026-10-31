# アーキテクチャ

実装の構造と設計判断をまとめる。要件は [requirements.md](requirements.md)、パラメータの意味は [parameters.md](parameters.md)、現在地は [status.md](status.md) が正典。

最終更新: 2026-10-31

## 1. 技術スタック

| 役割 | 選択 |
|---|---|
| GPU | wgpu(WGSL compute + render) |
| ウィンドウ / 入力 | winit(egui 経由) |
| UI | egui / eframe / egui-wgpu(`PaintCallback` でキャンバス描画) |
| 混色 | mixbox(依存は `crates/pigment` だけ = 依存グラフで隔離) |
| ホットリロード | notify(ファイル監視) |
| シリアライズ | serde + serde_json(プリセット・パレット・色・ストローク記録・作品メタ) |

依存バージョンは **egui の対応バージョンを起点に wgpu / winit をロックステップで固定**する。Rust は mise 管理(`mise exec -- cargo run`)。**workspace 構成**(R1、下記 §2)。

## 2. モジュール構成

**workspace(R1)**: CPU 純粋部を 3 crate に切り出し、GPU / UI はバイナリ crate に残す。`cargo tree` で mixbox 依存が `pigment` crate だけなことを機械的に確認でき、商用化時の差し替え範囲がこの crate に閉じる(plan.md §4)。km / paint-core は wgpu をリンクせず `cargo test -p <crate>` が数秒で回る。

```
kasaneiro/                (workspace ルート = バイナリ crate。[profile.*] もここ)
├─ Cargo.toml             [workspace] + [package] kasaneiro(GPU / UI / 入力)
├─ crates/
│  ├─ km/src/lib.rs       Kubelka-Munk 純関数の CPU 参照実装(依存ゼロ、cargo test 対象)
│  ├─ pigment/src/lib.rs  Palette(ランタイム4スロット、M5)・mixbox latent / 物性 uniform(mixbox 隔離点)
│  └─ paint-core/         CPU 純粋部(依存は bytemuck + serde のみ)
│     └─ src/
│        ├─ lib.rs        crate ルート(sim / brush / paper / replay / tool を公開)
│        ├─ sim.rs        SimParams(全パラメータの唯一の置き場)・Splat・CANVAS_SIZES(M8)
│        ├─ brush.rs      ストローク → splat 列(位置補間、筆圧の線形補間)
│        ├─ paper.rs      紙ハイトテクスチャの CPU 生成(値ノイズ4成分)
│        ├─ replay.rs     ストローク記録・再生モデル(H5。Recorder / Player)
│        └─ tool.rs       ツールの階層 enum(R2。Tool = Wet/Raster、gpu_id、ToolInfo)
├─ src/                   バイナリ crate(egui / wgpu / naga はここだけ)
│  ├─ main.rs             eframe 起動
│  ├─ app/                egui UI
│  │  ├─ mod.rs           PaintApp の状態・ライフサイクル・二層ディスパッチャ tool_panel・App::ui/save(R4)
│  │  │                   (App::ui のパネル順: Panel::top("menu_bar") → left("tools") → right("layers") → CentralPanel → file_modals)
│  │  │                   (dev_mode:通常/開発モードの二層化。eframe storage に永続化。has_painted:初回ガイド)
│  │  ├─ linehist.rs      線画の多段 Undo/Redo 履歴(M4.5d。RasterStroke / LineHistory / 再ラスタライズ)
│  │  └─ ui/              UI 状態 + パネル描画を per-file 分割(R4。impl PaintApp を分散)
│  │     ├─ mod.rs        UI 状態(PresetUi / WorkUi / PaletteUi / ReplayUi)+ 共通部品 NamedStore
│  │     ├─ tools.rs      乾燥ボタン+開発モードトグルと、アクティブレイヤーごとのツールパネル(dry_controls /
│  │     │                 active_tools_panel が brush/pencil/pen/highlight/dried を Frame::group で囲んで出し分け)
│  │     ├─ palette.rs    顔料パレット編集(palette_panel=色ライブラリ+パレット(4色一式)。水彩レイヤー選択時のみ表示。「色をつくる」(選択中スロットの色・名前・ρ/ω/γ/μ 編集。編集対象は brush_channel 連動)はツールバー色スウォッチのダブルクリックで開くモーダル color_edit_modal)
│  │     ├─ layers.rs     右パネルのレイヤースタック(layer_stack_panel。選択=ツール系統切替・可視性・並べ替え・合成方式)
│  │     ├─ tuning.rs     制作者向けの調整・診断・シミュ制御(tuning_dev_panel。開発モード時のみ表示)
│  │     ├─ panels.rs     表示/記録再生/シェーダー状態+status_bar(view/replay/shader_status/status_bar)
│  │     ├─ file_menu.rs  上部メニューバー(ファイル+ヘルプ)+モーダル(menu_bar / file_modals。ファイル=新規キャンバス・作品(保存/開く統合)・設定プリセット・PNG 書き出し・全部消す。ヘルプ=かさねいろについて(バージョン+ライセンス表示。exe 単体配布のためライセンス文同梱の正典)。新規キャンバス・全部消す・同名作品への上書き・描きかけを破棄しての作品読込はモーダル内の明示ボタンで確認)
│  │     └─ canvas.rs     キャンバス描画・初回ガイド・エラーオーバーレイ(canvas_ui / error_overlay)
│  ├─ gpu/                GpuCanvas。リソース定義と型・実行時メソッドを持ち、長い処理は分離
│  │  ├─ mod.rs           型定義(GpuCanvas / Pipelines / DriedLayer)・COMPUTE_SHADERS 表・
│  │  │                   clear/sync_layers/bake_dry/fast_dry/rewet/rebuild_pipelines
│  │  │                   (layer_palettes = 乾燥レイヤーの記録時パレットの CPU 正典。
│  │  │                    bake_dry(palette) が latent 焼き込みと対で push、不変条件 len==layers.len())
│  │  ├─ init.rs          GpuCanvas::new(テクスチャ・バッファ・bind group の生成)
│  │  ├─ callback.rs      CanvasCallback(フレーム描画。パス実行順の正典)
│  │  ├─ snapshot.rs      GpuCanvas::snapshot(PNG・スポイト用の読み戻し。H6)
│  │  ├─ persist.rs       GpuCanvas::export_state / import_state(作品保存の GPU 読み戻し/書き戻し。M7)
│  │  ├─ prelude.rs       Rust と WGSL で共有する定数(MAX_LAYERS / LATENT_TOTAL / TILE_SIZE 等)と
│  │  │                   それを WGSL の const にする shader_prelude(tests/shader_compile.rs も #[path] で共用)
│  │  ├─ shader_error.rs  WGSL コンパイルエラーの行番号補正の純関数+テスト(R3 QoL)
│  │  └─ hot_reload.rs    WGSL ファイル監視と再ビルド(H1)
│  ├─ input.rs            PointerSource trait(MouseSource / PenSource = egui Touch)
│  ├─ preset.rs           SimParams ⇄ JSON(H3)
│  ├─ replay.rs           ストローク記録の永続化(assets 依存の保存/読込。モデルは paint-core を再エクスポート。記録時パレット同梱の StoredRecording)
│  ├─ palette_store.rs    パレット(pigment::Palette)⇄ JSON(M5d。assets.rs の JSON ストア共通関数を使う)
│  ├─ pigment_store.rs    色単体(pigment::Pigment 1個)⇄ JSON(M5f。保存名=顔料名の1本化)
│  ├─ work.rs             作品保存(M7)。全状態を独自バイナリ1ファイル works/*.kasane に保存/読込
│  │                      (decode がメタの layer_palettes と層数の一致・レイヤー構成・末尾の余分なバイトを検査=
│  │                       GpuCanvas::layer_palettes の不変条件の入口。現行形式のみ読む)
│  └─ assets.rs           基準ディレクトリ解決(base_dir: 通常= CARGO_MANIFEST_DIR / embed-assets = exe 隣)
│                         と配布ビルドの既定 JSON 書き出し(seed_default_assets)。
│                         全ストア共通の保存名正規化(sanitize_name)・アトミック書き込み(.tmp → rename)・
│                         JSON ストアの json_save / json_load / json_delete / json_list
├─ tests/shader_compile.rs  WGSL コンパイル可能性テスト(naga。assets/shaders を走査し全キャンバスサイズで検証)
├─ tests/sim_params_layout.rs  SimParams の Rust 定義と common.wgsl の struct の一致検査
└─ assets/
   ├─ shaders/*.wgsl      実行時ロード(通常ビルドは埋め込まない。配布用 embed-assets のみ埋め込み)
   ├─ presets/*.json      SimParams プリセット(git 管理)
   ├─ strokes/*.json      テストストローク(git 管理。記録時パレット同梱=StoredRecording)
   ├─ palettes/*.json     顔料パレット・ライブラリ(git 管理。M5d。4件を同梱、default は default_palette() と同値)
   ├─ pigments/*.json     色単体ライブラリ(顔料1個=1ファイル。git 管理。M5f。定番の水彩絵具を含む24色を同梱)
   ├─ fonts/              UI フォント M PLUS 1p(OFL。include_bytes で埋め込み)
   └─ licenses/third-party.txt  依存クレートのライセンス一覧(git 管理。include_str で埋め込み、
                          「かさねいろについて」が表示。再生成コマンドはファイル冒頭)
   (works/*.kasane は作品保存の出力先。ユーザーの制作物なので snapshots/ 同様 git 管理外。M7)
```

`replay` の**モデル**(Recorder / Player / Recording)は paint-core、**永続化**(strokes の save/load、assets ディレクトリ解決に依存)はバイナリ crate、と分けてある。パスの基準は `assets::base_dir()` に1本化: 通常ビルドは `env!("CARGO_MANIFEST_DIR")`(リポジトリ直下。どこから起動しても効く)、配布ビルド(feature `embed-assets`)は **exe のあるディレクトリ**(ポータブルアプリ流儀。CWD 非依存)。assets/ works/ snapshots/ screenshots/ すべてこの直下。これを使うコードはバイナリ crate に置く。

## 3. GPU リソース

キャンバス = シミュレーション解像度。**M8 でサイズ可変化(R9 の値化)**: 選択肢は `CANVAS_SIZES = [512, 1024, 2048]`(正方形のみ)、既定は `DEFAULT_CANVAS_SIZE = 512`(試行錯誤は軽いサイズで回す)。実行時の値は `GpuCanvas.size` が正典で、全テクスチャ・dispatch・読み戻しの寸法がここから決まる。**サイズ変更 = GpuCanvas の作り直し**(`PaintApp::recreate_canvas`。テクスチャ・タイルバッファ・bind group が全てサイズ依存のため部分再構築より単純で安全。旧インスタンスは callback_resources の差し替えで drop)。UI はファイルメニューの「新規キャンバス…」(サイズ選択+確認)。64 の倍数に限定しているので読み戻し(snapshot/persist)の行バイト数は常に 256B 整列=パディング不要。用途は「広い紙」(テクセル密度据え置き)なので `diffuse_iters` 等のテクセル単位パラメータは再調整不要(plan §3 M8)。VRAM は 2048² で計 ~1.2GB(乾燥レイヤー8枚 512MB が支配的。窮屈なら `MAX_LAYERS` 削減)。テクスチャ・バッファは作成時に wgpu がゼロ初期化する(`GpuCanvas::new` では明示クリアしない)。「全部消す」(`clear`)と線画 Undo(`clear_line`)のゼロ化は、`Features::CLEAR_TEXTURE`(main.rs がアダプタ対応時だけ要求)があれば `encoder.clear_texture`、なければゼロ埋めの `write_texture`。

| テクスチャ | 形式 | 内容 |
|---|---|---|
| 水 | rgba32float × ping-pong 2枚 | r=水量 / g=速度x / b=速度y / a=濡れマスク(wet-area mask) |
| 浮遊顔料 | rgba32float × 2 | rgba 各チャンネル = 顔料1種(PIGMENTS と 1:1)。水の流れで移流する |
| 沈着顔料 | rgba32float × 2 | 同上。紙に定着した分で移流しない |
| 紙ハイト | r32float × 1(静的) | r = 高さ 0..1(0=谷 / 1=山)。起動時に CPU 生成、ping-pong しない |
| 乾燥レイヤー | rgba32float texture array(最大8スライス) | 1スライス = 1乾燥レイヤー、rgba = 4顔料濃度。RGB に潰さず顔料濃度のまま焼くので表示は毎フレーム latent で発色できる |
| 線画(M4.5a/c) | r32float × 3(鉛筆・ペン・ハイライト、静的な read_write) | r = インク濃度/不透明度 0..1。ping-pong せず `linesplat.wgsl` が read_write storage で直接蓄積。display は sampled で読み色の上に合成。清書ペンは compute 側も binding 10 で読む(M4.5b の透水率境界) |
| 湿レイヤー退避(M6) | rgba32float × 3(水・浮遊・沈着、COPY のみ) | 水彩ストローク開始時に `current` の3テクスチャを GPU 間コピーで退避。Ctrl+Z(水彩=1段 undo)で `current` へ書き戻す。bind せずコピーの読み元/書き先にしかならない。最新1本ぶんだけ保持 |

アクティブタイル(M6)は**テクスチャでなく storage バッファ**を2本持つ: `raw_active` / `active`(いずれも `array<u32>`、要素数 = (size/16)²。512²で1024個・2048²で16384個)。tilescan がタイルごとの生フラグを raw に書き、tiledilate が1タイル膨張して active を確定する。バッファ実体は bind group が保持する(GpuCanvas のフィールドには持たない)。

**作品保存の読み戻し用途(M7)**: 全状態を1ファイルに焼くため、乾燥レイヤー array・線画3枚・顔料 latent uniform に `COPY_SRC`(読み戻し)/ `COPY_DST`(書き戻し)を付けてある(湿レイヤーは undo 退避のため COPY 付き)。`copy_texture_to_buffer` に実体が要るため、乾燥レイヤーはスライスビューとは別に実体テクスチャを `dried_texture` として保持する。保存の読み戻しは全テクスチャのコピーを1回の submit にまとめ、完了待ちも1回。`export_state`/`import_state`([src/gpu/persist.rs](../src/gpu/persist.rs))が「GPU ⇄ f32 配列」を担い、parity は保存せず読込時に `current=0` へ正規化する(次フレームの sim が 0 を読んで 1 へ書くので ping-pong の一貫性は保たれる)。

**ping-pong は3テクスチャまとめて単一の `current`** で管理する。各 compute パスは3枚の src を読み、3枚の dst を必ず全テクセル書いて(変更しない分は素通し)反転する。パスごとに index を分けるより単純で、素通しコストはアクティブタイル(M6)が濡れ面積比例に絞る。線画テクスチャ(M4.5a/c)は ping-pong せず read_write で自己更新するため `current` に依存しない。

compute の binding は種別ごとに5レイアウト(R3 の `ComputeLayout`):
- **共通**(splat 系ほか、[assets/shaders/common.wgsl](../assets/shaders/common.wgsl)): `0/1` 水 src/dst、`2/3` 浮遊 src/dst、`4/5` 沈着 src/dst、`6` SimParams uniform、`7` splat storage、`8` 紙ハイト、`9` 顔料個性 uniform、`10` 清書ペンの線画(M4.5b、透水率境界。読むのは velocity/diffuse だけで他パスは宣言せず素通し)、`11` アクティブタイル有効フラグ(M6、storage read。splat/velocity/relax/flowout/advect/diffuse/transfer が読み、非アクティブなタイルを素通しする。fastdry/rewet は宣言せず全面計算)
- **bake**: 共通の `0..6, 8` + `9` = 乾燥レイヤーの書き込みスライス(splats なし)
- **raster**(M4.5a/c、linesplat.wgsl): `0` 対象の線画テクスチャ(read_write r32float)、`1` SimParams、`2` splat storage、`3` 紙ハイト。描画先(鉛筆/ペン/ハイライト)は bind group を差し替えて選ぶ(パイプラインは1本)。ライブ描画は流体パスがペン線を sampled で読むため `linesplat` を専用 compute パスに分ける(同一パス内で read_write と sampled を兼ねると使用範囲が衝突するため)
- **tilescan**(M6、tilescan.wgsl): `0` 水(read。current 側を parity 別 bind group で)、`3` SimParams、`4` splat storage、`5` raw_active(write)
- **tiledilate**(M6、tiledilate.wgsl): `0` raw_active(read)、`1` active(write)、`2` SimParams(common.wgsl の pressure_curve が参照するため束ねるだけ・未使用)

display の binding: `0/1/2` 水/浮遊/沈着、`3` SimParams、`4` 顔料 latent、`5` 紙ハイト、`6` 乾燥レイヤー array、`7` LayerUniform、`8/9` 線画(鉛筆/ペン、M4.5a)、`10` ハイライト(M4.5c)、`11` ViewUniform(M6、パン/ズーム/回転)、`12` アクティブタイル有効フラグ(M6、storage read。表示モード7の可視化)。

**ビューポート変換(binding 11、M6)**: `ViewUniform { center: vec2f, span: f32, cos_t: f32, sin_t: f32, _pad×3 }`(32B。WGSL の末尾パディングは vec3f だと align 16 で 48B に膨らむため f32×3 で詰めて Rust 側と一致させる)。fs_main が画面 uv → キャンバス uv を `canvas_uv = center + R(θ)·(画面uv − 0.5)·span`(`span = 1/zoom`、`R(θ)` は表示中心まわりの回転)で写してからサンプルするため、拡大/パン/**回転**が全サンプル(水/浮遊/沈着/紙/乾燥/線画)に一括で効く。SimParams とは分けた display 専用 uniform で(プリセット H3・記録 H5 を汚さない)、`CanvasCallback` がフレームごとに `write_buffer`。app 側で `zoom ∈ [1, 32]`。**回転で窓の隅がキャンバス外に出るぶん(`canvas_uv` が [0,1] の外)は `BG_COLOR`(紙の周りの机)で塗る**。クランプは回転なしなら窓をキャンバス内に(`center` を各軸 `[half, 1−half]`)、回転時は `center` のみ `[0,1]` に留める。ポインタ→テクセル変換(描画・スポイト)も同じ写像 `PaintApp::screen_to_texel`(逆回転込み)を通す。**スポイト(M5e)の snapshot 読み戻しは画面 uv で索引する**(`GpuCanvas::snapshot` はビュー変換を引数に取り、スポイトは画面と同じビューで焼くため。テクセル索引では拡大・回転時にズレる)。PNG 書き出し(H6)は `ViewUniform::default()`(全体・無回転)で焼くので、ズーム・回転中でもキャンバス全体が出る。snapshot が書いた view_buffer は次フレームの `CanvasCallback` が表示用に書き直す。回転操作は Shift+ホイールで15°刻み・「ビュー」パネルのスライダーで自由角。パン は中ボタンまたはスペース+左ドラッグ。

**顔料 latent(binding 4)のレイアウト(M5c、`array<vec4f, LATENT_TOTAL=78>`)**: 先頭 `GLOBAL_LATENTS=6` vec4 = パレット非依存のグローバル光学(`[0,1]`紙 / `[2,3]`白 R_w / `[4,5]`黒 R_b)。以降は**パレット枠**を `PALETTE_SLOTS = MAX_LAYERS+1 = 9` 個並べ、各枠 `PIGMENT_LATENTS=8` vec4(顔料4種 × c0..c3/RGB残差)。枠 `0..MAX_LAYERS-1` は乾燥レイヤーのスロット別、枠 `LIVE_PALETTE=MAX_LAYERS` は現行(湿レイヤー)のパレット。**乾かすと現行パレットの色を対応スロット枠へ焼き込む**(`GpuCanvas::record_layer_palette`)ため、顔料を後から編集しても乾燥済みレイヤーの色は変わらない。編集時は `set_palette` が physics(ρ/ω/γ/μ、全レイヤー共通)と live 枠だけを `write_buffer`(パイプライン再構築不要)。定数は [crates/pigment](../crates/pigment/src/lib.rs) と [src/gpu/prelude.rs](../src/gpu/prelude.rs) が正典で、WGSL へは `shader_prelude` が const として渡す(display.wgsl はベタ書きしない)。WGSL の `pal_base()` が対応。**M5h**: latent 枠は色しか持たないため、`bake_dry(…, palette)` が同時に `GpuCanvas::layer_palettes`(index=slot)へ Palette を丸ごと clone する。こちらが名前・ρ/ω/γ 込みの**正典**(GPU 枠は表示用キャッシュ)で、乾燥層パネルの「パレット抽出」UI が読む。

## 4. フレームの流れ

egui-wgpu の `CallbackTrait` で駆動する(`gpu/callback.rs` の `CanvasCallback`):

```
prepare(毎フレーム):
  SimParams を uniform に write_buffer(スライダー即時反映)
  splat storage buffer へヘッダ+splat 列を毎フレーム書く(0 個でもヘッダは書く。一時停止中でもブラシは反映。
    上限 MAX_SPLATS=8192、超過分は警告ログを出して切り捨て。fluid_count は線画ツールのフレームで 0)
  ブラシ入力パス:
    ラスタツール(M4.5a/c、line_target=Some)→ 専用 line_pass で linesplat.wgsl(対象の線画テクスチャへ直描き。水は注入しない)
    流体ツール → sim_pass 先頭で splat.wgsl(水+初速+顔料の注入。tool で分岐)
  アクティブタイル(M6): sim_pass 先頭(splat より前)で tilescan → tiledilate。
    濡れ面積+ブラシから active フラグを作り、以降の splat/sim 各パスが非アクティブなタイルを素通し。
    active_tiles=0 なら tilescan が全タイルを有効化=全面計算に戻る
  sim_steps 回(H6 の速度倍率):
    速度更新(velocity。ペン線で速度/にじみ拡張に透水率 M4.5b) → 発散緩和(relax)× relax_iters
    → FlowOutward(edge_eta > 0 のときだけ) → 移流(advect: 水+浮遊顔料)
    → 顔料+水の拡散(diffuse。水は毛細管拡散 note/07。ペン線で隣接流束に透水率 M4.5b)× diffuse_iters → 吸着/脱着+蒸発(transfer)
paint:
  display.wgsl でフルスクリーン三角形を描画(合成・発色・デバッグ表示)
```

ボタン駆動の単発パス: **bake**(乾かす=焼き込み)/ **fastdry**(水だけ除去)/ **rewet**(全面再湿潤)。PNG スナップショット(H6)は display と同じシェーダーでオフスクリーンに焼いて読み戻す(ビュー変換なし=キャンバス全体)。**UI スクショ(H6、AI レビュー用)**は GPU でなく egui 経路: `ViewportCommand::Screenshot` で画面全体(パネル込み)の撮影を要求し、eframe が次フレーム以降に返す `Event::Screenshot`(`Arc<ColorImage>`)を `poll_ui_screenshot`([app/mod.rs](../src/app/mod.rs))が受け取って固定パス `screenshots/ui-latest.png` へ上書き保存する。AI が同じパスから最新 UI を読めるようにするのが目的で、キャンバス PNG(タイムスタンプ命名)と別物。撮影のトリガーは2経路 — UI ボタンと、**外部ファイル監視**(`ScreenshotWatcher`([gpu/hot_reload.rs](../src/gpu/hot_reload.rs))が `screenshots/request-shot` の作成/変更を notify で拾う。AI が Bash からこのファイルを書けば実行中アプリに撮影させられる。自身が書く `ui-latest.png` はファイル名で除外し撮影ループを防ぐ)。

**作品保存(M7)のファイル形式**: プリセット等の軽い JSON と違い、作品は数十 MB の生 f32 テクスチャを含むため独自バイナリ1ファイル `works/*.kasane`(git 管理外)にする。先頭に `MAGIC "KSN1"` + メタ長 + メタ JSON(SimParams・現行パレット・レイヤー構成 `[slot, visible]`・レイヤーごとパレット・canvas_size・layer_count)を置き、続けて生 f32 ブロブを固定順(湿レイヤー3 → 乾燥レイヤー → 線画3 → 顔料 latent)で並べる([src/work.rs](../src/work.rs) の `encode`/`decode`)。**読込時は canvas_size(メタで `CANVAS_SIZES` を検証)が現在と違えば、そのサイズでキャンバスを作り直してから復元する**(M8。作品ファイルはサイズをまたいで持ち運べる)。線画の Undo 履歴(ストローク列)はテクスチャがあれば復元不要なので保存しない(読込直後の Undo/水彩1段 undo は効かない=履歴を破棄)。ファイル入出力を1モジュールに閉じ、将来 Web 版で保存先を差し替える余地を残す(plan §4)。

### シェーダー一覧(assets/shaders/)

| ファイル | 役割 |
|---|---|
| common.wgsl | 共通定義(SimParams / Splat 構造体、濡れ判定、バイリニア補間)。Rust 側が各シェーダーの先頭に連結してコンパイル。さらに前に Rust と共有する const(TILE_SIZE / TILES_PER_SIDE / MAX_LAYERS / LAYER_ORDER_VEC4S / GLOBAL_LATENTS / PIGMENT_LATENTS / LIVE_PALETTE / LATENT_TOTAL)を `shader_prelude`(gpu/prelude.rs)が生成して連結する |
| splat.wgsl | ブラシ入力。tool(描画/リフト/消去/ぼかし筆/吸い取り)で分岐。描画は分離色(brush_mix)で brush_channel / brush_channel2 の2チャンネルへ顔料を按分注入 |
| velocity.wgsl | 速度更新(水面勾配 = 水深 + paper_amp×紙ハイト → 加速)。ペン線の透水率 `perm` を速度場・にじみ拡張に掛ける(M4.5b) |
| relax.wgsl | 発散の反復緩和(δ = −ξ·div。濡れセルのみ、乾いたセルは壁) |
| flowout.wgsl | FlowOutward(縁の水を抜く)。既定オフ=edge_eta 0 で dispatch ごと省略。エッジダークニングは bake の dry_edge が担う |
| advect.wgsl | セミラグランジアン移流(水+浮遊顔料。差し替え用に分離) |
| diffuse.wgsl | 浮遊顔料+水の拡散(フィックの法則の陽解法。濡れセル間のみ・保存則あり)。顔料は粒の細かさ μ(顔料個性の w 成分)で per-channel に拡散倍率が変わる=分離色の主役。水は毛細管拡散(max^γ 重み、note/07)=置いた水が濡れた紙を伝って広がる。ペン線を挟む隣接流束に透水率を掛ける(M4.5b) |
| transfer.wgsl | 吸着/脱着(顔料個性 ρ/ω で per-channel 変調)+蒸発 |
| bake.wgsl | 「乾かす」= 乾燥レイヤーへの焼き込み+湿レイヤー全ゼロ |
| fastdry.wgsl | Fast Dry(浮遊→沈着に落として水と流れをゼロに。焼き込まない) |
| rewet.wgsl | Wet the Layer(全面マスク=1+rewet_water。沈着は既存の脱着で再浮遊) |
| linesplat.wgsl | ラスタ線画(M4.5a/c)。鉛筆/ペン/ハイライトを対象の r32float テクスチャへ直描き(line_mode で視覚分岐、line_eraser で減算)。流体を通らない |
| tilescan.wgsl | アクティブタイル(M6)第1段。タイル(16²)ごとに濡れマスクとブラシ近傍(流体へ流す splat)の有無を走査し raw_active に 0/1 を書く。active_tiles=0 で全タイル有効 |
| tiledilate.wgsl | アクティブタイル(M6)第2段。raw_active を3×3で膨張(1タイルの余裕)して active を確定する |
| display.wgsl | 表示: 層内発色(mixbox latent)→ レイヤー合成(multiply / KM)→ 線画合成(鉛筆→ペン→ハイライト、M4.5a/c)→ sRGB。デバッグ表示 H4 の分岐もここ(モード7=アクティブタイル可視化) |

## 5. 混色・発色のアーキテクチャ(2段構え)

**層「内」の混色 = mixbox、層「間」の合成 = Kubelka-Munk** と役割を分ける(RGB 3ch の K,S で層内を混ぜると黄+青が濁り、mixbox を使う意味が消えるため)。

- **CPU 側**([crates/pigment/src/lib.rs](../crates/pigment/src/lib.rs)): グローバル光学(紙+白+黒)の latent と、`Palette` の顔料 latent を計算して uniform で渡す。パレットは起動時に eframe storage から復元(無い・壊れていれば既定パレット)、以降 UI 編集のたびに再計算(M5)。**mixbox 呼び出しの隔離点**(CC BY-NC 対策、plan.md §4)
- **GPU 側**(display.wgsl): 画素ごとに4顔料の濃度比で latent を線形混合 → latent→RGB 多項式(mixbox eval_polynomial の WGSL 移植)で発色。紙とは被覆率 `1−exp(−pigment_density·総濃度)` で混合
- **KM 合成**: 各層(紙 → 乾燥レイヤー下から → 湿レイヤー)を白地・黒地に置いた mixbox 発色 R_w, R_b から `R = R_b`、`T² = (R_w−R_b)(1−R_b)` の閉じた形で反射率・透過率を導き、リニア色空間で下から光学合成(sinh/cosh 不要 = オーバーフロー対策不要)。この簡約が Curtis 一般式と一致することは [crates/km/src/lib.rs](../crates/km/src/lib.rs) のテストで担保。「顔料ごとの K,S プリセット」は不採用(混色モデルを二重化しないため)

## 6. 乾燥とレイヤー

- 湿レイヤーは常に1枚。「乾かす」= bake パスで `(浮遊+沈着) × dry_shift × 粒状感ゲート × (1 + dry_edge×縁バンド)` を texture array の新スライスへ書き、湿レイヤーを全ゼロに
- レイヤーの重ね順・可視性は `LayerUniform`(order + visible_mask)で display へ渡す。UI の並べ替えがそのまま KM 合成順になる
- 焼き込みは一方通行(乾燥レイヤーの再編集はしない)。Fast Dry / Wet the Layer が「焼かずに止める / 濡らし直す」の中間操作を提供する

## 6.5 線画(M4.5)

下書き鉛筆・清書ペン・白ハイライトのラスタツール。流体を通らず専用テクスチャに直描きする(型階層は R2 の `Tool::Raster`)。

- **描画経路**: 選択中ツールが `Tool::Raster` のとき `PaintApp::line_target()` が `Some(LineTarget)`(鉛筆/ペン/ハイライト)を返し、`CanvasCallback` がブラシ入力を `splat.wgsl` でなく `linesplat.wgsl` へ回す。対象テクスチャは Rust 側の bind group で選び、シェーダーは共通1本。`line_mode` が視覚分岐(鉛筆=柔エッジ・紙目粒状・筆圧→濃さ / ペン=硬エッジ・筆圧→太さ / ハイライト=硬めエッジの不透明白・筆圧→不透明度)、`line_eraser` で減算。ライブ描画は流体パスと別 compute パス(read_write と sampled の使用範囲衝突回避)
- **蓄積モデル**: 目標濃度への `max`(1フレーム内で密にサンプルしても一定線濃度へ収束)。r32float の read_write storage で自己更新(ping-pong 不要)
- **合成位置**: display.wgsl で色(紙→乾燥→湿)を合成した後、`apply_lines()` が鉛筆(グレー)→ ペン(濃色)→ ハイライト(白)の順にアルファ合成。`show_pencil`/`show_pen`/`show_highlight` で各レイヤーを非表示にできる。plan の合成順(紙→乾燥→湿→線画→ハイライト)そのまま
- **境界効果(M4.5b、透水率)**: 清書ペン濃度から `perm = 1 − line_block×ペン濃度` を出し、velocity(速度場・にじみ拡張)と diffuse(隣接流束)に掛ける。ブラシの直接スプラットには掛けない(線を跨ぐ筆使いなら越えられる)。`line_block=0` で境界なし。ペン線画テクスチャを共通レイアウトの binding 10 で sampled として読む
- **多段 Undo/Redo(M4.5d、[src/app/linehist.rs](../src/app/linehist.rs))**: 流体を通らないので決定論的に再ラスタライズできる。ストローク単位で生ポインタ点+実効 SimParams を `LineHistory` に保持し、Undo = 対象テクスチャを `clear_line` してから残りを `rasterize_line` で引き直す / Redo = 取り消し分を再適用(Ctrl+Z / Ctrl+Shift+Z、Redo は Ctrl+Y も)。保存済みパラメータで引き直すのでスライダーを変えても過去の線は不変。長い線は MAX_SPLATS 単位に分割して dispatch。湿レイヤー(水彩)は対象外(M6 の 1 段 undo)
- **記録との関係**: H5 のストローク記録は流体ツールのみ対象(ラスタは `line_target` が Some の間 recorder をスキップ)

## 6.6 アクティブタイル(M6)

シミュレーションコストを「紙の広さ」比例から「濡れ面積」比例へ絞る土台(M8 の 2048² 化の前提)。**タイル早期リターン方式**で、ping-pong 構造を崩さずに実装している。

- **2段のタイル判定**: `tilescan.wgsl` がタイル(16×16 テクセル)ごとに、タイル内に濡れたセル(濡れマスク a > 0.5)があるかと、このフレームのブラシ(流体へ流す splat の位置)の近傍かを走査して生フラグ `raw_active` を作る。乾いたセルは各シミュパスが素通しにするので、顔料が載っているだけ(Fast Dry 後など)のタイルは立てない。splat バッファのヘッダは毎フレーム書き(0 個でも)、`fluid_count` は線画ツールのフレームで 0 になる(線画の splat で流体のタイルを立てない)。`tiledilate.wgsl` が 3×3 で膨張(1タイル=16px の余裕)して `active` を確定する。膨張は「濡れ前線が1フレームに進む距離 < TILE_SIZE」を保証するための halo(既定 vel_max×sim_steps ≪ 16px)
- **ゲート**: 各シミュパス(splat/velocity/relax/flowout/advect/diffuse/transfer)は共通レイアウトの binding 11 で `active` を読み、**非アクティブなタイルは水/浮遊/沈着の3テクスチャを src→dst に素通しして return**(ping-pong の両バッファを常に一致させるため、計算を省いても必ず書く)。`tile_index_of`(common.wgsl)でテクセル→タイル添字に写す
- **実行順とハザード**: sim_pass の先頭で tilescan → tiledilate → splat → sim ループを同一 compute パスに積む。tiledilate が書いた `active` を後続パスが読む storage RAW は、テクスチャ ping-pong と同様に wgpu が自動バリアで解決する
- **退避と可視化**: `active_tiles=0` で tilescan が全タイルを有効化=全面計算(A/B・不具合時の退避)。display のモード7が `active` を可視化(計算中=緑 / 素通し=暗く + タイル格子)して「コストが濡れ面積比例か」を目視できる
- **SimParams とは別に view(パン/ズーム)を持つのと同じく、これは表示でなく計算範囲のノブ**。TILE_SIZE / TILES_PER_SIDE の WGSL 定数はキャンバスサイズ依存(M8)のため、gpu/prelude.rs の `shader_prelude` が生成して連結する(Rust 側定義が唯一の正典)

## 7. 入力の経路

```
ペン → Windows Ink(WM_POINTER)→ winit(GetPointerPenInfo で筆圧)→ egui::Event::Touch{force}
  → input.rs PenSource → PointerEvent(論理ピクセル+筆圧 0..1)
  → paint_core::brush(ストローク補間。サンプル間隔は筆圧反映後の実効半径 SimParams::radius_at)
  → Splat 列 → splat storage buffer → splat.wgsl
```

- `PointerSource` trait で Mouse / Pen を抽象(将来の wasm Pointer もここ)。ペン接地中はマウス入力を無視(egui-winit が Touch からポインタをエミュレートするため二重ストローク防止)
- **筆の含み**: 塗る筆(Wet::Paint)を置いたまま動かないフレームは、含み(`brush_charge` 秒。筆を下ろすたび満たす)が残る間、CPU(`PaintApp::feed_charge`)が毎フレーム **feed splat**(`Splat.feed=1`)を1つ積む。splat.wgsl の描画ブランチが分岐し、一括注入の代わりに目標水位への水補充(max)+少量の顔料(splat.wgsl の定数 `CHARGE_PIGMENT`)を注ぎ続け、その水を毛細管拡散(diffuse.wgsl、note/07)が外へ運ぶ=置くだけで色水が流れ出て広がる。ストローク記録(H5)は生ポインタ点のみなので feed は再生で再現されない(既知の制約)
- 筆圧マッピングは `実効値 = 基準値 × mix(1, 筆圧^γ, 効き)` を半径・水量・顔料量に適用(splat.wgsl と CPU 側で同式)
- ストローク記録(H5)は splat 列ではなく**補間前の生ポインタ入力**を保存 — 再生時にブラシ半径等を変えて同一ストロークで A/B 比較できる

## 8. ホットリロードとパラメータ反映(試行錯誤ループの実装)

- WGSL は `assets/shaders/` から実行時ロード。notify で監視し、保存で `rebuild_pipelines()`。コンパイルエラー時は `pipelines = None` にして描画をスキップ(クラッシュしない)+エラーオーバーレイ表示、直前の正常なパイプラインで続行
- **リリース配布ビルドのみ例外**: feature `embed-assets`(`cargo build --release --features embed-assets`)で exe 単体配布にする。(1) `assets/shaders/` 全体を `include_dir!` でバイナリへ埋め込み(取得元の分岐は `GpuCanvas::shader_source` の1箇所。ファイル監視は無効 = ShaderWatcher が早期 return)、(2) 既定 JSON アセット(pigments/palettes/presets/strokes)も埋め込み、初回起動時に `assets::seed_default_assets()` が exe 隣へ書き出す(ディレクトリ単位で「無ければ書く」— ユーザーの編集・削除を復活させない)、(3) パス基準が exe ディレクトリになる(`assets::base_dir`)。通常ビルドには include_dir はコンパイルされない(optional 依存)
- compute パイプラインは `COMPUTE_SHADERS` の表(WGSL ファイル名 → レイアウト種別)を回して作る(R3。**シェーダー追加 = 表に1行**)。名前引き `Pipelines::compute("splat.wgsl")` で参照し、パス実行順は `prepare()` のハードコードが正典。コンパイルエラーの行番号は common.wgsl 連結分ずれるので `remap_shader_error_lines` で補正して表示(R3。純関数、cargo test 対象)
- SimParams / Splat の WGSL 構造体定義は common.wgsl に1箇所化し、Rust 側で各シェーダーの先頭に連結 — 「パラメータ追加 = WGSL 1行」をシェーダーが増えても維持。**連結は3段**(M8): `shader_prelude`(Rust と共有する const を Rust が生成。gpu/prelude.rs)+ common.wgsl + シェーダー本体。行番号補正(R3)はプレリュード込みのプレフィックス行数で行う
- SimParams は毎フレーム uniform へ書くのでスライダーは即時反映。WGSL uniform 規則(16 バイト整列)に合わせる。現在は 56 フィールド = 224B でパディングなし。足すときの手順は [parameters.md](parameters.md) §11、Rust と WGSL の struct の一致は [tests/sim_params_layout.rs](../tests/sim_params_layout.rs) が検査する

## 9. テスト戦略

`cargo test --workspace` で全48件:

| 対象 | 件数 | 内容 |
|---|---|---|
| km crate | 6 | KM 純関数(簡約 vs 一般式の一致・K,S 往復・透明/不透明/グレーズ・K=0 で NaN にならない) |
| pigment crate | 6 | 黄+青=緑、latent 往復、紙の latent 往復、物性範囲、範囲外の物性のクランプ、μ なし JSON の既定値 |
| paint-core crate | 9 | SimParams 整列・筆圧式、brush 補間、replay 往復・再生中のツール維持、tool の gpu_id 往復・ラスタ判定、paper 生成 |
| バイナリ crate | 25 | 保存名の正規化(5)・JSON ストア往復、シェーダーエラー行番号補正(3)、プリセット・パレット・色・ストロークの往復と同梱分の読込、同梱 default パレットとコード既定値の一致、作品ファイルの往復と破損・層数不一致・レイヤー構成不正・末尾余剰の拒否 |
| [tests/shader_compile.rs](../tests/shader_compile.rs) | 1 | WGSL コンパイル可能性。実行時ロードのため cargo build では壊れた WGSL を検出できない。naga(wgpu と同バージョン)で assets/shaders の全シェーダーを、全キャンバスサイズのプレリュード付きでパース+検証 |
| [tests/sim_params_layout.rs](../tests/sim_params_layout.rs) | 1 | SimParams の Rust 定義と common.wgsl の struct のフィールド名・順序・型の一致 |

- CPU 純粋部(km / pigment / paint-core)は wgpu をリンクしないので `cargo test -p <crate>` が数秒で回る
- 流体シェーダーの**挙動はテストしない**方針 — 数値の正しさより見た目なので、デバッグ表示(H4)+ストローク再生(H5)で診断する
