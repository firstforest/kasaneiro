# 実装状況

[plan.md](plan.md) のマイルストーン・装備(H1〜H6)に対する現在地。**マイルストーンの完了条件を目で確認したとき・装備を追加/変更したときに、このファイルを更新する**(コード変更と同じコミットに含めるのが望ましい)。

最終更新: 2026-07-05

## 現在地

**M1a〜M1d 実装済み・目視確認待ち。** 確認すること: ①置いた水たまりが広がり、流れが見える(M1a)②水の流れに乗って色が広がり、水が減る(蒸発)と色が定着する(M1b)③ハンザイエローとフタロブルーを隣接させて描くと、境界が濁らず緑に馴染む(M1c)④紙目に沿ったストリークと粒状感が出る(M1d。表示モード「紙ハイト」で紙自体も確認できる。確認用プリセット [m1d-paper-streak-only.json](../assets/presets/m1d-paper-streak-only.json))。**エッジダークニング(FlowOutward)は先送り**にした(下記 M1d 行参照)ので M1d の確認は紙目側だけでよい。ストローク再生(H5)の `two-color-adjacent` でパラメータ A/B 比較しながら調整するとよい。全部 OK なら ✅ にして M1.5(筆圧)へ。M1 の完了条件「先に置いた色の近くに別の色で描くと馴染んで綺麗なグラデーションになる」の調整滞在はここから。

## マイルストーン

| マイルストーン | 状態 | メモ |
|---|---|---|
| M0 実験ハーネス | ✅ 完了 | マウスで splat 描画、WGSL ホットリロード、スライダー即時反映まで確認 |
| M1a 水の浅水層 | 🔶 実装済み・目視確認待ち | 水テクスチャ(rgba32float: 水量+速度+濡れマスク)を splat → 速度更新 → 発散緩和(δ=−ξ·div)→ セミラグランジアン移流で ping-pong 更新。移流は差し替え用に [assets/shaders/advect.wgsl](../assets/shaders/advect.wgsl) に分離。**濡れ領域マスク**(Curtis の wet-area mask、a チャンネル)で水の移動を筆が通った領域内に制限: 乾いたセルは速度ゼロ・全パス素通し、濡れたセルの水深勾配は乾いた隣を自セル値で代用(Neumann 境界)、緩和の δ も乾いたセルでは 0(壁扱い)。にじみ拡張スライダー(`wet_expand`): 乾いたセルが濡れた隣の水量に比例してマスク値を蓄積し 0.5 超で濡れに昇格、0 で固定マスク。紙ハイトは M1d で別テクスチャに置く(a は使わない)。完了条件「置いた水たまりが広がり、流れが見える。ストローク領域の外へはにじまない」は目視判定 |
| M1b 顔料層(単顔料) | 🔶 実装済み・目視確認待ち | 浮遊顔料・沈着顔料テクスチャ(rgba32float ペア、rgba=4顔料枠だが M1b は r のみ)を追加し、テクスチャ3ペアを単一 current で ping-pong(各パスは変更しないテクスチャも素通しで write)。splat で浮遊層に顔料注入 → 移流([assets/shaders/advect.wgsl](../assets/shaders/advect.wgsl) で水と同じバックトレース)→ [assets/shaders/diffuse.wgsl](../assets/shaders/diffuse.wgsl) で浮遊顔料の**拡散**を反復(フィックの法則の陽解法。4近傍・濡れセル間のみ・水量平均で重み付け・保存則あり。1反復の係数は安定条件で 0.2 上限、速いにじみは `diffuse_iters` 反復で稼ぐ)→ [assets/shaders/transfer.wgsl](../assets/shaders/transfer.wgsl) で吸着(水が少ないほど強い)/脱着(水が多いほど強い)+蒸発。拡散は「水筆で描いた水路へ顔料が広がってグラデーションになる」動きを作る(移流だけだと水が顔料を押しのける一方向の動きになる)。パラメータ追加: `brush_pigment` / `deposit_rate` / `lift_rate` / `evap_rate` / `pigment_diffuse` / `diffuse_iters`。通常表示は顔料の Beer-Lambert 風レンダリング(単顔料フタロブルー風、M1c で mixbox に置換)。完了条件「水の流れに乗って色が広がる。水が減ると色が定着する」は目視判定 |
| M1c Mixbox 混色 | 🔶 実装済み・目視確認待ち | 4顔料化: 浮遊/沈着テクスチャの rgba 4チャンネルを顔料スロットに割当([src/pigment.rs](../src/pigment.rs) の PIGMENTS: ハンザイエロー / フタロブルー / キナクリドンマゼンタ / バーントシェンナ、mixbox 推奨顔料色)。ブラシは `brush_channel` で注入先を選択(UI に色見本ボタン)。移流・拡散・吸着/脱着は M1b 時点で vec4 処理だったため変更なし。**混色は LUT の WGSL 移植を回避する分担**: CPU 側([src/pigment.rs](../src/pigment.rs)、mixbox 呼び出しの隔離点 = plan.md §4)で顔料基本色+紙色の latent を起動時に1回計算して uniform で渡し、GPU 側([assets/shaders/display.wgsl](../assets/shaders/display.wgsl))は画素ごとに濃度比で latent を線形混合 → latent→RGB 多項式(mixbox eval_polynomial の WGSL 移植)で発色。紙とは被覆率 1−exp(−`pigment_density`·総濃度) で latent 混合。CPU テスト([src/pigment.rs](../src/pigment.rs)): 黄+青=緑、latent 復元の一致。パラメータ追加: `brush_channel` / `pigment_density`。完了条件「黄+青を隣接させて緑に馴染む」は目視判定 |
| M1d FlowOutward + 紙ハイト | 🔶 実装済み・目視確認待ち | **紙ハイト**: CPU 生成の静的テクスチャ([src/paper.rs](../src/paper.rs)、値ノイズ3成分 = 低周波の沈殿ムラ+高周波の紙目+横方向の繊維ストリーク、r32float、compute binding 8 / display binding 5)。Curtis の3箇所適用: ①速度更新の水面 = 水深 + `paper_amp`×高さ(谷へ流れる)②にじみ拡張(`wet_expand`)を紙目で変調(`paper_wet`。前線が谷を選んで進み縁が不規則に)③吸着を凹部で強化(`paper_gran`、粒状化)。**FlowOutward**: [assets/shaders/flowout.wgsl](../assets/shaders/flowout.wgsl) 新設(発散緩和の後・移流の前)。濡れマスクのボックスぼかし M' で `p ← p − η·dt·(1−M')`、縁の水が減る → 勾配が縁向き → 移流が顔料を縁へ運ぶ。`edge_eta`=0 で dispatch ごと省略。パラメータ追加: `paper_amp` / `paper_gran` / `paper_wet` / `edge_eta` / `edge_radius`(SimParams は uniform の 16B 整列のためパディング3個付き、serde(default) で古いプリセットも読める)。**FlowOutward は先送り(既定 `edge_eta=0` でオフ、パス dispatch ごと省略=ゼロコスト)**: 今の弱い定式化(縁の水を線形に抜くだけ)では顔料が縁でなく中心へ寄り、エッジダークニングにならない。本物のコーヒーリングに要る接触線ピン留め+外向き毛細管流が足りず、乾いて縮む濡れ域への内向きドリフトに負ける。ちゃんとした乾燥が入る **M2 で再検討**(コード・スライダー・定式化メモは残置)。完了条件は「紙目に沿ったストリーク・粒状感が出る」に縮小し目視判定 |
| M1.5 筆圧(octotablet) | ⬜ 未着手 | |
| M2 乾燥とレイヤー | ⬜ 未着手 | |
| M3 削り・顔料個性・KM 合成 | ⬜ 未着手 | |
| M4 仕上げ(任意) | ⬜ 未着手 | |

## 装備(試行錯誤ループ)

| # | 装備 | 状態 | 実装場所 |
|---|---|---|---|
| H1 | WGSL ホットリロード | ✅ 完了 | [src/gpu/hot_reload.rs](../src/gpu/hot_reload.rs)、エラーオーバーレイは [src/app.rs](../src/app.rs) |
| H2 | パラメータパネル | ✅ 完了 | [src/sim/mod.rs](../src/sim/mod.rs) の `SimParams` → uniform buffer → スライダー |
| H3 | プリセット保存/読込 | ✅ 完了 | [src/preset.rs](../src/preset.rs)(SimParams ⇄ JSON)+ [src/app.rs](../src/app.rs) の保存/読込 UI。`assets/presets/*.json` は git 管理([default.json](../assets/presets/default.json) 同梱)。SimParams の `#[serde(default)]` でパラメータが増えても古いプリセットが読める。同梱プリセットが全部読めるかは cargo test で検査 |
| H4 | デバッグ表示切替 | ✅ 完了 | [assets/shaders/display.wgsl](../assets/shaders/display.wgsl) で分岐(通常=顔料 / 水量ヒートマップ / 速度場 / 湿りオーバーレイ=濡れ領域を青重ね / 浮遊顔料 / 沈着顔料 / 紙ハイト)、モード選択は [src/app.rs](../src/app.rs) |
| H5 | ストローク記録・再生 | ✅ 完了 | [src/replay.rs](../src/replay.rs)。記録するのは splat 列ではなく**補間前の生ポインタ入力**(フレーム番号+テクセル座標+筆圧、顔料スロットはストローク単位)なので、再生時にブラシ半径等を変えて同一ストロークで A/B 比較できる。再生はキャンバスをリセットして記録時と同じテンポで流し込む。`assets/strokes/*.json` は git 管理(2色隣接の [two-color-adjacent.json](../assets/strokes/two-color-adjacent.json) 同梱) |
| H6 | シミュレーション制御 | ✅ 完了 | 一時停止 / 1ステップ実行 / 速度倍率(ステップ/フレーム)/ キャンバスリセット / PNG スナップショット([src/app.rs](../src/app.rs)。display と同じシェーダーでオフスクリーンに焼いて読み戻し = [src/gpu/mod.rs](../src/gpu/mod.rs) `snapshot()`。`snapshots/` にタイムスタンプ+プリセット名で保存、git 対象外) |

## plan.md §2 構成との差分

計画上のファイルでまだ存在しないもの:

- `src/input.rs` — 入力抽象 trait。M1.5(octotablet)まで不要、現状はマウス直結
- `src/km.rs` — M3 と同時(CPU 参照実装 + cargo test)

## 計画外でやったこと

- 日本語フォント対応: egui デフォルトフォントに日本語グリフがないため、Windows システムフォント(游ゴシック等)をフォールバック登録([src/app.rs](../src/app.rs) `install_japanese_font`)
- アセットディレクトリ解決の共通化: shaders / presets / strokes が同じ規則(CARGO_MANIFEST_DIR 基準)を使うため [src/assets.rs](../src/assets.rs) に `asset_dir()` を置いた
- WGSL 共通定義の連結ロード: SimParams / Splat の struct 定義を [assets/shaders/common.wgsl](../assets/shaders/common.wgsl) に1箇所化し、Rust 側で各シェーダーの先頭に連結してコンパイル。「パラメータ追加 = WGSL 1行」をシェーダーが増えても維持するため
- WGSL コンパイル可能性テスト: 実行時ロードのため cargo build では壊れた WGSL を検出できない。naga(wgpu と同バージョン)でパース+検証する [tests/shader_compile.rs](../tests/shader_compile.rs) を追加(挙動はテストしない方針のまま、コンパイル可否だけ守る)
