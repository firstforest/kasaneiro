# アーキテクチャ

実装の構造と設計判断をまとめる。要件は [requirements.md](requirements.md)、パラメータの意味は [parameters.md](parameters.md)、現在地は [status.md](status.md) が正典。

最終更新: 2026-07-05(M3 完了・M4 進行中。R1 workspace 化を適用)

## 1. 技術スタック

| 役割 | 選択 |
|---|---|
| GPU | wgpu(WGSL compute + render) |
| ウィンドウ / 入力 | winit(egui 経由) |
| UI | egui / eframe / egui-wgpu(`PaintCallback` でキャンバス描画) |
| 混色 | mixbox(依存は `crates/pigment` だけ = 依存グラフで隔離) |
| ホットリロード | notify(ファイル監視) |
| シリアライズ | serde + serde_json(プリセット・ストローク記録) |

依存バージョンは **egui の対応バージョンを起点に wgpu / winit をロックステップで固定**する。Rust は mise 管理(`mise exec -- cargo run`)。**workspace 構成**(R1、下記 §2)。

## 2. モジュール構成

**workspace(R1)**: CPU 純粋部を 3 crate に切り出し、GPU / UI はバイナリ crate に残す。`cargo tree` で mixbox 依存が `pigment` crate だけなことを機械的に確認でき、商用化時の差し替え範囲がこの crate に閉じる(plan.md §4)。km / paint-core は wgpu をリンクせず `cargo test -p <crate>` が数秒で回る。

```
my-paint/                 (workspace ルート = バイナリ crate。[profile.*] もここ)
├─ Cargo.toml             [workspace] + [package] my-paint(GPU / UI / 入力)
├─ crates/
│  ├─ km/src/lib.rs       Kubelka-Munk 純関数の CPU 参照実装(依存ゼロ、cargo test 対象)
│  ├─ pigment/src/lib.rs  顔料パレット定義・mixbox latent / 物性 uniform(mixbox 隔離点)
│  └─ paint-core/         CPU 純粋部(依存は bytemuck + serde のみ)
│     └─ src/
│        ├─ lib.rs        crate ルート(sim / brush / paper / replay / tool を公開)
│        ├─ sim.rs        SimParams(全パラメータの唯一の置き場)・Splat・CANVAS_SIZE
│        ├─ brush.rs      ストローク → splat 列(位置補間、筆圧の線形補間)
│        ├─ paper.rs      紙ハイトテクスチャの CPU 生成(値ノイズ3成分)
│        ├─ replay.rs     ストローク記録・再生モデル(H5。Recorder / Player)
│        └─ tool.rs       ツールの階層 enum(R2。Tool = Wet/Raster、gpu_id、ToolInfo)
├─ src/                   バイナリ crate(egui / wgpu / naga はここだけ)
│  ├─ main.rs             eframe 起動
│  ├─ app/                egui UI
│  │  ├─ mod.rs           PaintApp の状態・ライフサイクル・ディスパッチャ tool_panel・App::ui(R4)
│  │  └─ ui/              UI 状態 + パネル描画を per-file 分割(R4。impl PaintApp を分散)
│  │     ├─ mod.rs        UI 状態(PresetUi / ReplayUi)+ 共通部品 NamedStore
│  │     ├─ tools.rs      乾燥ボタン・水ブラシ(dry_controls / brush_panel)
│  │     ├─ layers.rs     レイヤー可視性・並べ替え・合成方式(layer_panel / layers_panel)
│  │     ├─ tuning.rs     乾燥・筆圧・味付け・診断・シミュ制御(tuning_panel)
│  │     ├─ panels.rs     プリセット/記録再生/シェーダー状態(preset/replay/shader_status)
│  │     └─ canvas.rs     キャンバス描画とエラーオーバーレイ(canvas_ui / error_overlay)
│  ├─ gpu/                GpuCanvas。リソース定義と型・実行時メソッドを持ち、長い処理は分離
│  │  ├─ mod.rs           型定義(GpuCanvas / Pipelines / DriedLayer)・COMPUTE_SHADERS 表・
│  │  │                   clear/sync_layers/bake_dry/fast_dry/rewet/rebuild_pipelines
│  │  ├─ init.rs          GpuCanvas::new(テクスチャ・バッファ・bind group の生成)
│  │  ├─ callback.rs      CanvasCallback(フレーム描画。パス実行順の正典)
│  │  ├─ snapshot.rs      GpuCanvas::snapshot(PNG 読み戻し。H6。R8 で readback を一般化予定)
│  │  ├─ shader_error.rs  WGSL コンパイルエラーの行番号補正の純関数+テスト(R3 QoL)
│  │  └─ hot_reload.rs    WGSL ファイル監視と再ビルド(H1)
│  ├─ input.rs            PointerSource trait(MouseSource / PenSource = egui Touch)
│  ├─ preset.rs           SimParams ⇄ JSON(H3)
│  ├─ replay.rs           ストローク記録の永続化(assets 依存の保存/読込。モデルは paint-core を再エクスポート)
│  └─ assets.rs           assets/ ディレクトリ解決(CARGO_MANIFEST_DIR 基準なのでバイナリ crate に残す)
├─ tests/shader_compile.rs  WGSL コンパイル可能性テスト(naga)
└─ assets/
   ├─ shaders/*.wgsl      実行時ロード(ビルドに埋め込まない)
   ├─ presets/*.json      SimParams プリセット(git 管理)
   └─ strokes/*.json      テストストローク(git 管理)
```

`replay` の**モデル**(Recorder / Player / Recording)は paint-core、**永続化**(strokes の save/load、assets ディレクトリ解決に依存)はバイナリ crate、と分けてある。`asset_dir` が `env!("CARGO_MANIFEST_DIR")` でワークスペースルート基準の `assets/` を指すため、これを使うコードはバイナリ crate に置く必要がある。

## 3. GPU リソース

キャンバス = シミュレーション解像度 512²(`CANVAS_SIZE`)。

| テクスチャ | 形式 | 内容 |
|---|---|---|
| 水 | rgba32float × ping-pong 2枚 | r=水量 / g=速度x / b=速度y / a=濡れマスク(wet-area mask) |
| 浮遊顔料 | rgba32float × 2 | rgba 各チャンネル = 顔料1種(PIGMENTS と 1:1)。水の流れで移流する |
| 沈着顔料 | rgba32float × 2 | 同上。紙に定着した分で移流しない |
| 紙ハイト | r32float × 1(静的) | r = 高さ 0..1(0=谷 / 1=山)。起動時に CPU 生成、ping-pong しない |
| 乾燥レイヤー | rgba32float texture array(最大8スライス) | 1スライス = 1乾燥レイヤー、rgba = 4顔料濃度。RGB に潰さず顔料濃度のまま焼くので表示は毎フレーム latent で発色できる |

**ping-pong は3テクスチャまとめて単一の `current`** で管理する。各 compute パスは3枚の src を読み、3枚の dst を必ず全テクセル書いて(変更しない分は素通し)反転する。パスごとに index を分けるより単純で、512² では素通しコストは十分軽い。

compute の binding は全シェーダー共通([assets/shaders/common.wgsl](../assets/shaders/common.wgsl)):
`0/1` 水 src/dst、`2/3` 浮遊 src/dst、`4/5` 沈着 src/dst、`6` SimParams uniform、`7` splat storage、`8` 紙ハイト、`9` 顔料個性 uniform(bake.wgsl のみ 9 が書き込みスライス)。

## 4. フレームの流れ

egui-wgpu の `CallbackTrait` で駆動する(`gpu/callback.rs` の `CanvasCallback`):

```
prepare(毎フレーム):
  SimParams を uniform に write_buffer(スライダー即時反映)
  splat があれば storage buffer へ(一時停止中でもブラシは反映)
  splat パス(水+初速+顔料の注入。tool で分岐)
  sim_steps 回(H6 の速度倍率):
    速度更新(velocity) → 発散緩和(relax)× relax_iters
    → FlowOutward(edge_eta > 0 のときだけ) → 移流(advect: 水+浮遊顔料)
    → 顔料拡散(diffuse)× diffuse_iters → 吸着/脱着+蒸発(transfer)
paint:
  display.wgsl でフルスクリーン三角形を描画(合成・発色・デバッグ表示)
```

ボタン駆動の単発パス: **bake**(乾かす=焼き込み)/ **fastdry**(水だけ除去)/ **rewet**(全面再湿潤)。PNG スナップショット(H6)は display と同じシェーダーでオフスクリーンに焼いて読み戻す。

### シェーダー一覧(assets/shaders/)

| ファイル | 役割 |
|---|---|
| common.wgsl | 共通定義(SimParams / Splat 構造体、濡れ判定、バイリニア補間)。Rust 側が各シェーダーの先頭に連結してコンパイル |
| splat.wgsl | ブラシ入力。tool(描画/リフト/消去/水筆/ならし)で分岐 |
| velocity.wgsl | 速度更新(水面勾配 = 水深 + paper_amp×紙ハイト → 加速) |
| relax.wgsl | 発散の反復緩和(δ = −ξ·div。濡れセルのみ、乾いたセルは壁) |
| flowout.wgsl | FlowOutward(縁の水を抜く。既定オフ=edge_eta 0、M2 方式に置き換え済みで残置) |
| advect.wgsl | セミラグランジアン移流(水+浮遊顔料。差し替え用に分離) |
| diffuse.wgsl | 浮遊顔料の拡散(フィックの法則の陽解法。濡れセル間のみ・保存則あり) |
| transfer.wgsl | 吸着/脱着(顔料個性 ρ/ω で per-channel 変調)+蒸発 |
| bake.wgsl | 「乾かす」= 乾燥レイヤーへの焼き込み+湿レイヤー全ゼロ |
| fastdry.wgsl | Fast Dry(浮遊→沈着に落として水と流れをゼロに。焼き込まない) |
| rewet.wgsl | Wet the Layer(全面マスク=1+rewet_water。沈着は既存の脱着で再浮遊) |
| display.wgsl | 表示: 層内発色(mixbox latent)→ レイヤー合成(multiply / KM)→ sRGB。デバッグ表示 H4 の分岐もここ |

## 5. 混色・発色のアーキテクチャ(2段構え)

**層「内」の混色 = mixbox、層「間」の合成 = Kubelka-Munk** と役割を分ける(RGB 3ch の K,S で層内を混ぜると黄+青が濁り、mixbox を使う意味が消えるため)。

- **CPU 側**([crates/pigment/src/lib.rs](../crates/pigment/src/lib.rs)): 顔料基本色+紙色+白+黒の mixbox latent を起動時に1回計算して uniform で渡す。**mixbox 呼び出しの隔離点**(CC BY-NC 対策、plan.md §4)
- **GPU 側**(display.wgsl): 画素ごとに4顔料の濃度比で latent を線形混合 → latent→RGB 多項式(mixbox eval_polynomial の WGSL 移植)で発色。紙とは被覆率 `1−exp(−pigment_density·総濃度)` で混合
- **KM 合成**: 各層(紙 → 乾燥レイヤー下から → 湿レイヤー)を白地・黒地に置いた mixbox 発色 R_w, R_b から `R = R_b`、`T² = (R_w−R_b)(1−R_b)` の閉じた形で反射率・透過率を導き、リニア色空間で下から光学合成(sinh/cosh 不要 = オーバーフロー対策不要)。この簡約が Curtis 一般式と一致することは [crates/km/src/lib.rs](../crates/km/src/lib.rs) のテストで担保。「顔料ごとの K,S プリセット」は不採用(混色モデルを二重化しないため)

## 6. 乾燥とレイヤー

- 湿レイヤーは常に1枚。「乾かす」= bake パスで `(浮遊+沈着) × dry_shift × 粒状感ゲート × (1 + dry_edge×縁バンド)` を texture array の新スライスへ書き、湿レイヤーを全ゼロに
- レイヤーの重ね順・可視性は `LayerUniform`(order + visible_mask)で display へ渡す。UI の並べ替えがそのまま KM 合成順になる
- 焼き込みは一方通行(乾燥レイヤーの再編集はしない)。Fast Dry / Wet the Layer が「焼かずに止める / 濡らし直す」の中間操作を提供する

## 7. 入力の経路

```
ペン → Windows Ink(WM_POINTER)→ winit(GetPointerPenInfo で筆圧)→ egui::Event::Touch{force}
  → input.rs PenSource → PointerEvent(論理ピクセル+筆圧 0..1)
  → brush.rs(ストローク補間。サンプル間隔は筆圧反映後の実効半径 SimParams::radius_at)
  → Splat 列 → splat storage buffer → splat.wgsl
```

- `PointerSource` trait で Mouse / Pen を抽象(将来の wasm Pointer もここ)。ペン接地中はマウス入力を無視(egui-winit が Touch からポインタをエミュレートするため二重ストローク防止)
- 筆圧マッピングは `実効値 = 基準値 × mix(1, 筆圧^γ, 効き)` を半径・水量・顔料量に適用(splat.wgsl と CPU 側で同式)
- ストローク記録(H5)は splat 列ではなく**補間前の生ポインタ入力**を保存 — 再生時にブラシ半径等を変えて同一ストロークで A/B 比較できる

## 8. ホットリロードとパラメータ反映(試行錯誤ループの実装)

- WGSL は `assets/shaders/` から実行時ロード。notify で監視し、保存で `rebuild_pipelines()`。コンパイルエラー時は `pipelines = None` にして描画をスキップ(クラッシュしない)+エラーオーバーレイ表示、直前の正常なパイプラインで続行
- compute パイプラインは `COMPUTE_SHADERS` の表(WGSL ファイル名 → レイアウト種別)を回して作る(R3。**シェーダー追加 = 表に1行**)。名前引き `Pipelines::compute("splat.wgsl")` で参照し、パス実行順は `prepare()` のハードコードが正典。コンパイルエラーの行番号は common.wgsl 連結分ずれるので `remap_shader_error_lines` で補正して表示(R3。純関数、cargo test 対象)
- SimParams / Splat の WGSL 構造体定義は common.wgsl に1箇所化し、Rust 側で各シェーダーの先頭に連結 — 「パラメータ追加 = WGSL 1行」をシェーダーが増えても維持
- SimParams は毎フレーム uniform へ書くのでスライダーは即時反映。WGSL uniform 規則(16 バイト整列)に合わせ、末尾の `_pad` を置き換えてからフィールドを増やす

## 9. テスト戦略

- **CPU 参照実装+cargo test**(`cargo test --workspace`): km crate(KM 純関数5件)、pigment crate(黄+青=緑、latent 往復、物性範囲)、paint-core(SimParams 整列・筆圧式、brush 補間、replay 往復、paper 生成)、プリセット互換・ストローク読込(バイナリ crate)
- **WGSL コンパイル可能性テスト**([tests/shader_compile.rs](../tests/shader_compile.rs)): 実行時ロードのため cargo build では壊れた WGSL を検出できない。naga(wgpu と同バージョン)で全シェーダーをパース+検証
- 流体シェーダーの**挙動はテストしない**方針 — 数値の正しさより見た目なので、デバッグ表示(H4)+ストローク再生(H5)で診断する
