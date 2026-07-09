# リファクタリング計画(完了記録)

作成 2026-07-05 / 見直し 2026-07-07。M4.5〜M8 を見据えた構造改善の計画で、**大規模な作り直しは不要、各マイルストーンで苦しくなる箇所を先回りで直す**という位置づけだった。マイルストーン M0〜M8 は全完了([status.md](status.md))。R1〜R4・R9 は挙動不変リファクタとして適用済み、**R5 は M5 実装に吸収されて達成**、**R6〜R8 は当のマイルストーンが別設計で先に完了したため未適用**(= 残った任意の技術的負債)。以下は各項目の狙いと実施/見送りの記録。

## 0. 壊さないもの(前提)

リファクタは「試行錯誤の速度最優先」の核を維持したまま行う:

- WGSL ホットリロード(H1)と common.wgsl 連結ロードの仕組み
- **「パラメータ追加 = フィールド1行+スライダー1行+WGSL 1行」**(H2)
- mixbox 呼び出しの隔離(plan §4)。R1 で「ファイル規約」から「依存グラフ」へ格上げ

## 1. 状態サマリ

| # | 項目 | 効く先 | 状態 |
|---|---|---|---|
| R1 | workspace 化(km / pigment / paint-core の切り出し) | 全体。mixbox 隔離・テスト反復の高速化 | ✅ 完了(2026-07-05) |
| R2 | Tool の階層 enum 化(`Tool::Wet` / `Tool::Raster`) | M4.5 全般 | ✅ 完了(2026-07-05) |
| R3 | パイプラインのテーブル駆動化+エラー行番号補正 | M4.5、日々のシェーダー反復 | ✅ 完了(2026-07-05) |
| R4 | app.rs の UI 分割と状態のグループ化 | M4.5 / M5 | ✅ 完了(2026-07-05) |
| R5 | 顔料バッファのフィールド化+ Palette 状態化 | M5b | ✅ 完了(2026-07-05、M5a/b に吸収) |
| R6 | LayerStack の抽出 | M4.5a / M5c | ⬜ 見送り(未適用のまま完了。任意の負債) |
| R7 | ストロークモデルの統一(replay と Undo 履歴) | M4.5d | ⬜ 見送り(別系統の undo で完了。既知の傷は解消済み) |
| R8 | 読み戻し(readback)ユーティリティの抽出 | M5e / M7 | ⬜ 見送り(inline readback 重複のまま完了。任意の負債) |
| R9 | CANVAS_SIZE の値化 | M6 / M8 | ✅ 完了(2026-07-07、M8 と同時) |

## 2. R1 — workspace 化(クレート切り出し)

**狙い**: mixbox 隔離(CC BY-NC 対策)を依存グラフで強制し、CPU 純粋部を wgpu 抜きで数秒テストできるようにする。切り出すのは `km`(Kubelka-Munk 純関数)・`pigment`(顔料・パレット・mixbox の唯一の依存点)・`paint-core`(SimParams / Splat / Tool / brush / replay / paper)の3つのみ。UI・GPU は残す。詳細な構成は [architecture.md](architecture.md) §2。

**実施メモ(2026-07-05 完了)**:

- ルート `Cargo.toml` を `[workspace]`(members = 3 crate)+ `[package] kasaneiro`(バイナリ)の二役にした。`[profile.dev]` はワークスペースルートでのみ有効なのでルートに残す。
- **replay はモデルと永続化を分けた**: モデル(Recorder / Player / Recording)を paint-core へ、ファイル保存/読込(assets ディレクトリ解決に依存)はバイナリ crate `src/replay.rs` に残し、そこで `pub use paint_core::replay::*;` で再エクスポート。`asset_dir` が `env!("CARGO_MANIFEST_DIR")` 基準なので、これを使うコード(replay 永続化・preset・assets)はバイナリ crate に置く必要がある。
- ルートが同時にバイナリ package なので `cargo test` だけでは下位 crate のテストが回らない。**`cargo test --workspace` / `cargo clippy --workspace` に変更**(CLAUDE.md 更新済み)。

## 3. R2 — Tool の階層 enum 化

**狙い**: `u32` 魔法数だったツールを `Tool::Wet(WetTool)` / `Tool::Raster{..}` の直和型にし、**型の階層に描画経路を埋め込む**(ラスタツールを splat.wgsl へ流す誤りを型で排除)。UI 表示は `ToolInfo` trait で共通化。`TOOLS` 定数表を廃止。置き場所は paint-core。

**実施メモ(2026-07-05 完了)**:

- [../crates/paint-core/src/tool.rs](../crates/paint-core/src/tool.rs) に `Tool` / `WetTool` / `RasterTool` / `ToolInfo` を実装。app は `PaintApp.tool: Tool` を選択の正典に持ち、`Tool::wet()` が `Some(WetTool)` のとき `wt.gpu_id()` を `params.tool` へ同期する(ラスタツールは `wet()==None` で流体経路に流れない)。ツールバーは `WetTool::ALL` を回して `label()`/`hint()` で描く。
- replay は on-disk 互換のため `RecordedStroke.tool: u32` のまま。enum への変換用に `WetTool::from_gpu_id` / `TryFrom<u32>` を用意しテスト済み。

## 4. R3 — パイプラインのテーブル駆動化+エラー行番号補正

**狙い**: `rebuild_pipelines()` の「シェーダー1本 = 5箇所手書き」を表駆動にして**「シェーダー追加 = ファイル名1行」**にする。加えて common.wgsl 連結でずれるコンパイルエラー行番号を補正する(ホットリロードの反復速度に直結)。

**実施メモ(2026-07-05 完了)**:

- `const COMPUTE_SHADERS: &[(&str, ComputeLayout)]`(キー = WGSL ファイル名)を回して compute パイプラインを `HashMap` に作り、`Pipelines::compute("splat.wgsl")` で名前引き。**シェーダー追加 = 表に1行**。display/snapshot は別レイアウト・2フォーマットなので表の外。
- パス実行順は `prepare()` のハードコードのまま(「ここがパス実行順の正典」とコメント明記)。
- 行番号補正 `remap_shader_error_lines`(gpu/shader_error.rs、純関数): プレフィックス行数を引く。codespan の2形式を対象、想定外フォーマットは素通し(fail-safe)。cargo test で3件検証。

## 5. R4 — app.rs の UI 分割と状態のグループ化

**狙い**: 約 340 行の `tool_panel()` を分割し、プリセット(H3)とストローク(H5)で重複する「名前入力+保存+一覧」を共通化。`PaintApp` の散らばったフィールドを `ReplayUi` / `PresetUi` に束ねる。

**実施メモ(2026-07-05 完了)**:

- `src/app.rs` → `src/app/mod.rs`、UI 状態を `src/app/ui/mod.rs` へ。パネル描画は `impl PaintApp` を `app/ui/` 配下に分散(`tools.rs` / `layers.rs` / `tuning.rs` / `panels.rs` / `canvas.rs`。可視性 `pub(in crate::app)`)。`tool_panel` は各セクションを呼ぶディスパッチャに縮小。
- フィールド束ね: `PresetUi { store }` / `ReplayUi { store, recorder, pending_recording, player }`。「名前入力+保存+一覧」は `NamedStore::save_controls` / `list_rows` に一本化。
- 同方針で **gpu/mod.rs も分割**(挙動不変の移動のみ、責務の再設計はなし):`init.rs`(`GpuCanvas::new`)・`callback.rs`・`snapshot.rs`・`shader_error.rs`。

## 6. R5 — 顔料バッファのフィールド化+ Palette 状態化

**狙い**: `GpuCanvas::new()` でローカル変数のまま破棄されていた顔料/物性バッファをフィールド化し(編集時に `write_buffer` するため)、UI の `const PIGMENTS` 直読みを `Palette` 構造体経由に変えて M5a/b をスライダー追加だけにする。

**実施メモ(2026-07-05、M5a/b に吸収)**: 独立コミットは切らず M5 実装の中で目標を満たした。

- 顔料/物性バッファは `GpuCanvas` のフィールド化済み(`physics_buffer` / `latents_buffer` = [gpu/mod.rs](../src/gpu/mod.rs)、生成は [gpu/init.rs](../src/gpu/init.rs))。`pigment_buffer` の名前は無くなり顔料 latent は `latents_buffer` に統合。
- ランタイム編集は `GpuCanvas::set_palette(&mut self, queue, &Palette)` が両バッファを `write_buffer` で更新 = R5 が想定した「編集時に再計算して write_buffer」そのもの。
- `const PIGMENTS` 直読みは廃止。UI・app・replay はすべて `pigment::Palette` の状態(`self.palette`)経由([crates/pigment/src/lib.rs](../crates/pigment/src/lib.rs)。JSON 保存対応)。R5 が想定した構造体が M5d でそのままライブラリ化された。

## 7. R6 — LayerStack の抽出

**狙い**: `GpuCanvas.layers: Vec<DriedLayer>`(public フィールドを UI が直接編集 → `sync_layers()` 手動呼び出しという規約結合)を、編集メソッドを持ち GPU 同期を内部化した `LayerStack` に閉じる。

**見送りメモ(2026-07-07)**: **未適用のまま M4.5a/M5c が完了**した。[gpu/mod.rs](../src/gpu/mod.rs) は現在も `pub layers: Vec<DriedLayer>` で規約結合が残っている。線画が流体レイヤーとは別の `line_textures` として実装されたため、想定した「レイヤー状態がここに集中して膨らむ」成長は起きず、痛みは限定的だった。**現状は任意の負債**: `sync_layers()` の呼び忘れが将来バグになりうるので、レイヤー編集 UI に手を入れるときに合わせて `LayerStack` 化するのが安い。単独で今やる必要はない。

## 8. R7 — ストロークモデルの統一

**狙い**: M4.5d の多段 Undo は「ストローク+当時の実効パラメータ」の再生で、H5 の `RecordedStroke`(現在値で再生)と目的が逆の兄弟。ストローク型を「points + tool + channel(+ Optional な captured params)」の共有モデルに整理し、M4.5d を Recording の再利用で書けるようにする。

**見送りメモ(2026-07-07)**: **統一モデルは採らず、M4.5d/M6 が別設計の undo で完了**した。多段 Undo は `undo_stack: Vec<UndoKind>`(操作順)+ ラスタ線画の多段履歴 `LineHistory`([app/linehist.rs](../src/app/linehist.rs))+ 湿レイヤー1段の GPU テクスチャ退避 `wet_backup`([gpu/mod.rs](../src/gpu/mod.rs))の三者構成。線画は実効パラメータを `LineHistory::begin` でスナップショットする形で「captured params」相当を別途満たしており、`RecordedStroke` は今も `channel + tool + points` のまま(統一していない)。この分岐は意図的な設計。

**既知の傷は解消済み(2026-07-07)**: `Player::advance` が再生中に `params.tool` / `brush_channel` を上書きする件は、`params.tool` は R2 の副作用で毎フレーム `self.tool` から再同期される([app/ui/tools.rs](../src/app/ui/tools.rs))ため元から自然治癒、残っていた `brush_channel`(再生後に選択顔料が最後のストロークの色へ飛ぶ)は `start_replay` で退避 → `stop_replay` で復元する形で修正([app/mod.rs](../src/app/mod.rs)、`ReplayUi.saved_channel`)。

## 9. R8 — 読み戻し(readback)ユーティリティの抽出

**狙い**: `snapshot()` の「テクスチャ → バッファ → map → Vec」を `gpu/readback.rs` へ汎用化(任意テクスチャ・任意サイズ・256B パディング対応)し、M5e(スポイト)と M7(作品保存)で再利用する。

**見送りメモ(2026-07-07)**: **汎用化せずに M5e/M7 が完了**した。読み戻しコードは共通化されず、[gpu/snapshot.rs](../src/gpu/snapshot.rs)(PNG 用)と [gpu/persist.rs](../src/gpu/persist.rs)(M7 作品保存)に inline で重複している。R9 でキャンバスサイズを 64 の倍数に限定したので **256B パディング計算は不要のまま**(汎用化の主目的の一つは前提ごと消えた)。**現状は任意の負債**: 読み戻し箇所がこれ以上増える、または R9 の 64 倍数制限を外すときに `gpu/readback.rs` へ寄せる。今の2〜3箇所の重複では割に合わない。

## 10. R9 — CANVAS_SIZE の値化

**狙い**: gpu/mod.rs・app.rs・paper.rs に波及する const `CANVAS_SIZE` を `GpuCanvas` のフィールド化し、M8 の設定化に備える。

**実施メモ(2026-07-07、M8 と同一コミットで適用)**: paint-core の const は `CANVAS_SIZES = [512, 1024, 2048]` + `DEFAULT_CANVAS_SIZE = 512` に置き換え、実行時の値は `GpuCanvas.size`(+`tiles_per_side`)が正典。app は写し `PaintApp.canvas_size` を持つ(座標変換・保存が renderer ロックなしで毎フレーム読むため。変更は `recreate_canvas` 経由のみ)。**パディング計算は入れず**、サイズを 64 の倍数に限定して `GpuCanvas::new` の assert で強制することで readback の行バイト数を常に 256B 整列にした。WGSL 側のタイル定数はサイズ依存になったため `shader_prelude`(gpu/mod.rs)が const 2行を生成して連結する(architecture.md §8 参照)。

## 11. 判断が要るもの(保留)

**SimParams にツール/UI 状態が混ざっている**: `tool` / `brush_channel` / `display_mode` / `display_gain` / `compose_mode` は物理調整値ではないのにプリセットへ保存される = プリセット読込で選択中ツール・顔料・デバッグ表示まで切り替わる。`#[serde(skip)]` を付ければ直せるが、「プリセット = 作業状態ごと復元したい」意図なら現状維持が正。**好みの問題なので気になったときに決める**。

## 12. やらないこと(plan.md §4 と同じ「先送り」の流儀)

| 論点 | 当面の選択 | 再検討のトリガー |
|---|---|---|
| パス実行の trait 抽象化 | やらない(`prepare()` の `run` クロージャが既に良い継ぎ目) | M6 アクティブタイルで dispatch を差し替えるとき、その場で必要な分だけ |
| WGSL の include 機構一般化 | やらない(common.wgsl 連結で足りている) | シェーダー間の共有関数が実際に増えたら |
| GpuCanvas の全面再設計 | やらない(リソース定義が長いだけで責務は一貫) | R3+R8+テクスチャ生成ヘルパーの抽出で不足を感じたら |
| crate のさらなる細分化 | やらない(R1 の3つまで) | 依存の隔離・純粋性の保証という明確な理由が新たに生まれたら |
