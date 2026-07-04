# 実装状況

[plan.md](plan.md) のマイルストーン・装備(H1〜H6)に対する現在地。**マイルストーンの完了条件を目で確認したとき・装備を追加/変更したときに、このファイルを更新する**(コード変更と同じコミットに含めるのが望ましい)。

最終更新: 2026-07-04

## 現在地

**M1a(水の浅水層)実装済み・目視確認待ち。「置いた水たまりが広がり、流れが見える」を目で確認したら ✅ にして M1b(顔料層)へ。**

## マイルストーン

| マイルストーン | 状態 | メモ |
|---|---|---|
| M0 実験ハーネス | ✅ 完了 | マウスで splat 描画、WGSL ホットリロード、スライダー即時反映まで確認 |
| M1a 水の浅水層 | 🔶 実装済み・目視確認待ち | 水テクスチャ(rgba32float: 水量+速度+濡れマスク)を splat → 速度更新 → 発散緩和(δ=−ξ·div)→ セミラグランジアン移流で ping-pong 更新。移流は差し替え用に [assets/shaders/advect.wgsl](../assets/shaders/advect.wgsl) に分離。**濡れ領域マスク**(Curtis の wet-area mask、a チャンネル)で水の移動を筆が通った領域内に制限: 乾いたセルは速度ゼロ・全パス素通し、濡れたセルの水深勾配は乾いた隣を自セル値で代用(Neumann 境界)、緩和の δ も乾いたセルでは 0(壁扱い)。にじみ拡張スライダー(`wet_expand`): 乾いたセルが濡れた隣の水量に比例してマスク値を蓄積し 0.5 超で濡れに昇格、0 で固定マスク。紙ハイトは M1d で別テクスチャに置く(a は使わない)。完了条件「置いた水たまりが広がり、流れが見える。ストローク領域の外へはにじまない」は目視判定 |
| M1b 顔料層(単顔料) | ⬜ 未着手 | |
| M1c Mixbox 混色 | ⬜ 未着手 | 完了条件: 黄+青を隣接させて緑に馴染む |
| M1d FlowOutward + 紙ハイト | ⬜ 未着手 | H3・H5 もここで整備 |
| M1.5 筆圧(octotablet) | ⬜ 未着手 | |
| M2 乾燥とレイヤー | ⬜ 未着手 | |
| M3 削り・顔料個性・KM 合成 | ⬜ 未着手 | |
| M4 仕上げ(任意) | ⬜ 未着手 | |

## 装備(試行錯誤ループ)

| # | 装備 | 状態 | 実装場所 |
|---|---|---|---|
| H1 | WGSL ホットリロード | ✅ 完了 | [src/gpu/hot_reload.rs](../src/gpu/hot_reload.rs)、エラーオーバーレイは [src/app.rs](../src/app.rs) |
| H2 | パラメータパネル | ✅ 完了 | [src/sim/mod.rs](../src/sim/mod.rs) の `SimParams` → uniform buffer → スライダー |
| H3 | プリセット保存/読込 | ⬜ 未着手 | M1d で。`assets/presets/` も未作成 |
| H4 | デバッグ表示切替 | ✅ 完了 | [assets/shaders/display.wgsl](../assets/shaders/display.wgsl) で分岐(通常 / 水量ヒートマップ / 速度場 / 湿りオーバーレイ=濡れ領域を青重ね)、モード選択は [src/app.rs](../src/app.rs)。顔料・紙ハイトのモードは M1b/M1d で追加 |
| H5 | ストローク記録・再生 | ⬜ 未着手 | M1d で。`src/replay.rs`・`assets/strokes/` も未作成 |
| H6 | シミュレーション制御 | ✅ 完了 | 一時停止 / 1ステップ実行 / 速度倍率(ステップ/フレーム)/ キャンバスリセット([src/app.rs](../src/app.rs))。PNG スナップショットのみ未実装(H3/H5 と同時期に) |

## plan.md §2 構成との差分

計画上のファイルでまだ存在しないもの:

- `src/input.rs` — 入力抽象 trait。M1.5(octotablet)まで不要、現状はマウス直結
- `src/replay.rs` — H5 と同時
- `src/km.rs` — M3 と同時(CPU 参照実装 + cargo test)
- `assets/presets/` / `assets/strokes/` — H3 / H5 と同時

## 計画外でやったこと

- 日本語フォント対応: egui デフォルトフォントに日本語グリフがないため、Windows システムフォント(游ゴシック等)をフォールバック登録([src/app.rs](../src/app.rs) `install_japanese_font`)
- WGSL 共通定義の連結ロード: SimParams / Splat の struct 定義を [assets/shaders/common.wgsl](../assets/shaders/common.wgsl) に1箇所化し、Rust 側で各シェーダーの先頭に連結してコンパイル。「パラメータ追加 = WGSL 1行」をシェーダーが増えても維持するため
- WGSL コンパイル可能性テスト: 実行時ロードのため cargo build では壊れた WGSL を検出できない。naga(wgpu と同バージョン)でパース+検証する [tests/shader_compile.rs](../tests/shader_compile.rs) を追加(挙動はテストしない方針のまま、コンパイル可否だけ守る)
