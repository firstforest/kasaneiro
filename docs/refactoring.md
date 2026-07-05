# リファクタリング計画

作成日: 2026-07-05。M4.5 以降のマイルストーン([plan.md](plan.md) §3)を見据えた構造改善の計画。ソース全体(src 13 ファイル・シェーダー 12 本)を M4.5〜M8 の要求と突き合わせた検討結果で、**大規模な作り直しは不要、ただし各マイルストーンで確実に苦しくなる箇所を先回りで直す**という位置づけ。

**適用の原則**: 各項目は該当マイルストーンの着手前に、**挙動不変のリファクタとして 1 コミットずつ**適用する(cargo test + naga 検証([../tests/shader_compile.rs](../tests/shader_compile.rs))で確認)。適用したら本ファイルの状態列と、影響があれば [architecture.md](architecture.md)(モジュール/クレート構成)を同じコミットで更新する。

## 0. 壊さないもの(前提)

リファクタはすべて「試行錯誤の速度最優先」の核を維持したまま行う:

- WGSL ホットリロード(H1)と common.wgsl 連結ロードの仕組み
- **「パラメータ追加 = フィールド1行+スライダー1行+WGSL 1行」**(H2)。SimParams が別クレートへ移っても、触るファイルの場所が変わるだけでこの定型は崩さない
- mixbox 呼び出しの隔離(plan §4)。R1 でむしろ「ファイル規約」から「依存グラフ」へ格上げする

## 1. 実施順サマリ

R1 は挙動不変のファイル移動が主なので**最初**にやる(後にやると R2〜R4 で動かしたコードをもう一度動かすことになる)。R5〜R9 は該当マイルストーンの直前で良い。

| # | 項目 | 効く先 | 規模 | 状態 |
|---|---|---|---|---|
| R1 | workspace 化(km / pigment / paint-core の切り出し) | 全体。mixbox 隔離の構造化・テスト反復の高速化 | 中 | ✅ 完了(2026-07-05) |
| R2 | Tool の階層 enum 化(`Tool::Wet` / `Tool::Raster`) | M4.5 全般 | 小 | ✅ 完了(2026-07-05) |
| R3 | パイプラインのテーブル駆動化+エラー行番号補正 | M4.5、日々のシェーダー反復 | 小〜中 | ⬜ 未着手 |
| R4 | app.rs の UI 分割と状態のグループ化 | M4.5 / M5 | 中 | ⬜ 未着手 |
| R5 | 顔料バッファのフィールド化+ Palette 状態化の準備 | M5b | 極小 | ⬜ 未着手 |
| R6 | LayerStack の抽出 | M4.5a / M5c | 小 | ⬜ 未着手 |
| R7 | ストロークモデルの統一(replay と Undo 履歴) | M4.5d | 小 | ⬜ 未着手 |
| R8 | 読み戻し(readback)ユーティリティの抽出 | M5e / M7 | 小 | ⬜ 未着手 |
| R9 | CANVAS_SIZE の値化 | M6 / M8 | 中 | ⬜ 未着手 |

## 2. R1 — workspace 化(クレート切り出し)

plan.md §2 の「単一クレートで開始、分割は必要になってから」を **2026-07-05 に「分割する」へ更新**した(本計画の決定)。切り出す価値が構造的にあるものは3つで、それ以外(UI・GPU)は切り出しても得がないため残す。

```
my-paint/                  (workspace)
├─ Cargo.toml              workspace 定義([profile.*] 設定もここへ移動)
├─ crates/
│  ├─ km/                  Kubelka-Munk 純関数。依存ゼロ
│  ├─ pigment/             顔料・パレット・latent/物性 uniform 計算。mixbox はここだけが依存
│  └─ paint-core/          SimParams / Splat / Tool / ストローク補間(brush)/
│                          記録再生モデル(replay)/ 紙ノイズ(paper)
│                          (依存は bytemuck + serde のみ。GPU に触らない)
└─ src/                    バイナリ crate(app / gpu / input / preset / assets)
                           egui / wgpu / naga はここだけ。tests/shader_compile.rs も残留
```

| crate | 切り出す理由 |
|---|---|
| `km` | 依存ゼロの純関数+テスト。もともと「CPU 参照実装」という独立した役割で境界が既に綺麗 |
| `pigment` | **mixbox 隔離(CC BY-NC 対策)が依存グラフで強制される**のが最大の利点。`cargo tree` で mixbox 依存がこの crate だけと機械的に確認でき、商用化時の差し替え = この crate の中身の置換になる |
| `paint-core` | CPU 純粋で相互に使い合う一塊(replay → brush → Splat)。wgpu をリンクせずに `cargo test -p paint-core` が数秒で回る |

副次効果: 今は km.rs を1行直してもバイナリ crate 全体が再コンパイルされるが、分割後は変更 crate とその依存先だけになる。日々一番触る app/gpu/WGSL は最上位なので、そこの編集で下位 crate は再ビルドされない。

**やり過ぎ防止**: これ以上細かく割らない(`paper` 単独 crate 等)。crate 境界は「依存の隔離(pigment)」「純粋性の保証(km, paint-core)」という明確な理由があるものだけ。

**同時に更新するドキュメント**: [plan.md](plan.md) §2(更新済み)・[architecture.md](architecture.md) §2 モジュール構成・CLAUDE.md のコマンド/構成記述。

**実施メモ(2026-07-05 完了)**:

- ルート `Cargo.toml` を `[workspace]`(members = 3 crate)+ `[package] my-paint`(バイナリ)の二役にした。`[profile.dev]` はワークスペースルートでのみ有効なのでルートに残す。
- **replay はモデルと永続化を分けた**: モデル(Recorder / Player / Recording)を paint-core へ、ファイル保存/読込(assets ディレクトリ解決に依存)はバイナリ crate `src/replay.rs` に残し、そこで `pub use paint_core::replay::*;` して再エクスポート。`asset_dir` が `env!("CARGO_MANIFEST_DIR")` でワークスペースルート基準の `assets/` を指すため、これを使うコード(replay 永続化・preset・assets)はバイナリ crate に置く必要がある。呼び出し側の `crate::replay::*` はそのまま動く(paint-core のモデルは serde だけに依存 = 目標どおり)。
- ルートが同時にバイナリ package なので、`cargo test` だけだと下位 crate のテストが回らない。**`cargo test --workspace` / `cargo clippy --workspace` に変更**(CLAUDE.md 更新済み)。
- km crate はどこからも依存されない純粋な参照/テスト crate(workspace メンバーなので `--workspace` でテストは回る)。

## 3. R2 — Tool の階層 enum 化

現状ツールは `u32` の魔法数で、[../src/app.rs](../src/app.rs) の `TOOLS` 定数表・[../assets/shaders/splat.wgsl](../assets/shaders/splat.wgsl) の分岐・[../src/replay.rs](../src/replay.rs) の記録が並走している。M4.5 の鉛筆(5)・ペン(6)・ハイライト(7)は**流体シミュを通らず別テクスチャに描く**ため、CPU 側で経路が分かれる。「wet 系か raster 系か」を実行時に聞くのではなく、**型の階層そのものに経路を埋め込む**:

```rust
/// ツール全体。トップレベルの分岐 = 描画経路の分岐(型が経路を保証する)
pub enum Tool {
    /// 流体シミュ経由(splat バッファ → splat.wgsl)
    Wet(WetTool),
    /// 線画テクスチャ直描き(M4.5。流体を通らない)
    Raster { kind: RasterTool, eraser: bool },
}

pub enum WetTool { Paint, Lift, Erase, WaterBrush, Smear }
pub enum RasterTool { Pencil, Pen, Highlight }   // M4.5 で追加

impl WetTool {
    /// SimParams::tool へ書く値。gpu_id を持つのは WetTool だけ —
    /// raster ツールを splat.wgsl へ流す誤りを型レベルで排除する
    pub fn gpu_id(self) -> u32 { /* 0..4 */ }
}

/// UI 表示用メタ情報。WetTool / RasterTool 両方に実装し、
/// ツールバー描画は共通コードでツール群を回すだけにする
pub trait ToolInfo {
    fn label(&self) -> &'static str;   // 「描画」など
    fn hint(&self) -> &'static str;    // ホバー文言
}
```

設計の要点:

- **enum(直和型)= 閉じた集合の分岐、trait = UI 表示の共通化**、と役割を分ける。閉じた集合に trait オブジェクトは使わない
- `match` の網羅性チェックで、M4.5 のツール追加時に処理漏れがコンパイルエラーになる
- 消しゴムは M4.5a の仕様(トグルで反転)どおり `Raster` 側のフィールド。wet 側に存在しない状態を型に載せる
- 置き場所は paint-core(R1)。ラベルは `&'static str` なので egui 依存は不要
- replay の保存形式は互換のため on-disk では `u32` のまま残し、`TryFrom<u32>` で enum へ変換(不正値はエラー)

これで `TOOLS` 定数表は消え、ラベル・文言・GPU 値・経路が enum の impl に一元化される。

**実施メモ(2026-07-05 完了)**:

- [../crates/paint-core/src/tool.rs](../crates/paint-core/src/tool.rs) に `Tool` / `WetTool` / `RasterTool` / `ToolInfo` を実装。`RasterTool` と `Tool::Raster` は型階層だけ先に用意し `#[allow(dead_code)]`(M4.5a で UI・実装を足すときに allow を外す)。
- app は `PaintApp.tool: Tool` を選択の正典に持ち、`Tool::wet()` が `Some(WetTool)` のとき `wt.gpu_id()` を `params.tool` へ同期して splat.wgsl の分岐に渡す(ラスタツールは `wet()==None` で流体経路に流れない)。`TOOLS` 定数表は廃止し、ツールバーは `WetTool::ALL` を回して `label()`/`hint()` で描く。
- replay は on-disk 互換のため `RecordedStroke.tool: u32` のまま(GPU 境界も u32 なので変換不要)。enum への変換が要る箇所(M4.5 / R7)向けに `WetTool::from_gpu_id` / `TryFrom<u32>` を用意しテスト済み。
- 網羅性チェックは M4.5 でラスタ経路を実装するとき(`Tool` を `match` する箇所)に効く。R2 時点では app は `wet()` の Option 経由なので `Tool::Raster` 追加は既存コードを壊さない。

## 4. R3 — パイプラインのテーブル駆動化+エラー行番号補正

[../src/gpu/mod.rs](../src/gpu/mod.rs) の `rebuild_pipelines()` はシェーダー1本につき「read → load → make_compute → struct フィールド → Pipelines 初期化」の5箇所を手書きしており、M4.5 で 3 本以上増える。

- `const COMPUTE_SHADERS: &[(&str, レイアウト種別)]` +名前引きのコレクションにして、**「シェーダー追加 = ファイル名1行」**にする(bake だけ専用レイアウトなので種別タグ付き)
- パス実行順(`prepare()` のシーケンス)はハードコードのまま維持する。ここは心臓部で、データ駆動化しても得がない
- **QoL**: common.wgsl 連結でコンパイルエラーの行番号がずれる問題は、エラー文字列中の行番号から共通部の行数を引いて表示し直すだけで直せる。ホットリロードの反復速度に直結するため R3 に同梱する

## 5. R4 — app.rs の UI 分割と状態のグループ化

[../src/app.rs](../src/app.rs)(842 行)の `tool_panel()` は1関数で約 340 行あり、M4.5(ラスタツール・消しゴムトグル・線画レイヤー表示・Undo UI)と M5(パレット編集 UI・ライブラリ)で倍増する。

- `src/ui/` サブモジュールへ分割: `tools.rs` / `layers.rs` / `presets.rs` / `tuning.rs`(畳んである味付けスライダー群)など
- **名前入力+保存+一覧のパターンがプリセット(H3)とストローク(H5)で完全に重複**しているので共通化する
- `PaintApp` のフィールドも束ねる: `recorder`/`pending_recording`/`player`/`stroke_name`/`stroke_list` → `ReplayUi`、`preset_name`/`preset_list` → `PresetUi`

## 6. R5 — 顔料バッファのフィールド化+ Palette 準備(M5b の前提)

`pigment_buffer` / `physics_buffer` は `GpuCanvas::new()` で**ローカル変数のまま bind group に渡して破棄**されている(bind group が生かすので動くが、後から `write_buffer` できない)。M5b「編集時に再計算して write_buffer」はフィールド化が前提。合わせて UI が `const PIGMENTS` を直接読んでいる箇所を `Palette` 構造体(中身は当面 const のコピー)経由に変えておくと、M5a/M5b がスライダー追加だけになる。

## 7. R6 — LayerStack の抽出(M4.5a / M5c の前)

`GpuCanvas.layers: Vec<DriedLayer>` は public フィールドで、UI が直接編集 → `sync_layers()` 手動呼び出しという規約結合。M4.5 で「並べ替え不可の固定レイヤー(下書き・清書・ハイライト)」、M5c で「レイヤーごとの latent 記録」が加わり、レイヤー状態はここが最も成長する。編集メソッドを持つ `LayerStack` に閉じ、GPU 同期(uniform 反映)を内部化する。

## 8. R7 — ストロークモデルの統一(M4.5d の前)

M4.5d の多段 Undo は「ストローク+**当時の実効パラメータ**」の履歴再生で、H5 の `RecordedStroke`(パラメータは現在値で再生)と目的が逆の兄弟。[../src/replay.rs](../src/replay.rs) のストローク型を「points + tool + channel(+ Optional な captured params)」の共有モデルに整理しておくと、M4.5d が Recording の再利用で書ける。置き場所は paint-core(R1)。

**ついでに直す既知の傷**: `Player::advance` が `params.tool` / `brush_channel` を上書きしたまま復元しないため、**再生後に選択中ツール・顔料が変わってしまう**。R7 で再生終了時に元の値へ戻す。

## 9. R8 — 読み戻しユーティリティの抽出(M5e / M7 の前)

`snapshot()` の「テクスチャ → バッファ → map → Vec」は、M5e(スポイト: カーソル下 1px)と M7(全シミュテクスチャ+乾燥レイヤーの保存/復元)でそのまま欲しくなる。`gpu/readback.rs` へ汎用化(任意テクスチャ・任意サイズ・bytes_per_row の 256B パディング対応)する。合わせて**永続テクスチャを1つの構造体(名前+フォーマット付き)で列挙可能**にしておくと、M7 の保存は「全テクスチャを列挙して読む/書く」だけになる。

## 10. R9 — CANVAS_SIZE の値化(M6 / M8 の前)

const `CANVAS_SIZE` は gpu/mod.rs(テクスチャ・dispatch・スナップショット)、app.rs(座標変換)、paper.rs に波及している。M8 で設定化するため、`GpuCanvas` のフィールド(+ app へ公開)へ先に変えておく。テクスチャが増える M4.5 以後にやるより今の方が安い。

**注意**: snapshot の bytes_per_row は 512/1024/2048 なら偶然 256B 整列を満たすが、値化するときにパディング計算を入れる(R8 の readback 汎用化と整合)。

## 11. 判断が要るもの(挙動変更を伴うため保留)

**SimParams にツール/UI 状態が混ざっている**: `tool` / `brush_channel` / `display_mode` / `display_gain` / `compose_mode` は物理調整値ではないのにプリセットへ保存される = プリセット読込で選択中ツール・顔料・デバッグ表示まで切り替わる。該当フィールドに `#[serde(skip)]` を付けるだけで直せる(GPU レイアウトは不変、「1行×3」の原則も維持)が、「プリセット = 作業状態ごと復元したい」という意図なら現状維持が正。**使い勝手の好みの問題なので、気になったときに決める**。

## 12. やらないこと(plan.md §4 と同じ「先送り」の流儀)

| 論点 | 当面の選択 | 再検討のトリガー |
|---|---|---|
| パス実行の trait 抽象化 | やらない(`prepare()` の `run` クロージャが既に良い継ぎ目) | M6 アクティブタイルで dispatch を差し替えるとき、その場で必要な分だけ |
| WGSL の include 機構一般化 | やらない(common.wgsl 連結で足りている) | シェーダー間の共有関数が実際に増えたら |
| GpuCanvas の全面再設計 | やらない(リソース定義が長いだけで責務は一貫) | R3+R8+テクスチャ生成ヘルパーの抽出で不足を感じたら |
| crate のさらなる細分化 | やらない(R1 の3つまで) | 依存の隔離・純粋性の保証という明確な理由が新たに生まれたら |
