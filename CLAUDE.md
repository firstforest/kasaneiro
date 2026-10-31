# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

**かさねいろ(kasaneiro)** — Rust + wgpu 製の水彩シミュレーションペイントツール(Windows 個人用ツール)。名前は「色を重ねる」+ 襲の色目から。3つのコア要件 = **wet-on-wet**(色が馴染むグラデーション)/ **グレージング**(乾いた色の上に綺麗に重なる)/ **削り**(リフト+完全消去の2ツール)。ドキュメント・コミットメッセージは日本語。

## ドキュメントの役割(作業開始時にまず status.md を確認する)

| ドキュメント | 役割 |
|---|---|
| [docs/status.md](docs/status.md) | **現在の実装状況の正典**(現在地の要約・マイルストーン表・装備表・既知の課題/要目視確認)。作業開始時にまず読む |
| [docs/changelog.md](docs/changelog.md) | 実装の経緯を時系列で残す変更ログ。現状の説明は status.md に書き、ここには混ぜない |
| [docs/requirements.md](docs/requirements.md) | 要件仕様(3要件の判定基準・ツール構成・UI/入力要件・非機能要件) |
| [docs/architecture.md](docs/architecture.md) | 実装構造(スタック・モジュール構成・テクスチャ・パス順序・シェーダー一覧・混色の2段構え) |
| [docs/parameters.md](docs/parameters.md) | 全パラメータ(`SimParams`)と顔料個性 ρ/ω/γ/μ のリファレンス(既定値・UI 範囲・意味・実効式) |
| [docs/plan.md](docs/plan.md) | 実装計画(マイルストーン M0〜M8・装備 H1〜H6・先送り判断 §4) |
| [docs/refactoring.md](docs/refactoring.md) | リファクタリング計画の完了記録(残る任意の負債 R6〜R8 と「やらないこと」表) |
| [docs/note/00-overview.md](docs/note/00-overview.md) | 技術調査ノートの起点。設計判断に迷ったら参照 |

**更新規則(コード変更と同じコミットで):**

- マイルストーン・装備(H1〜H6)を進めたとき、ツール・UI・ファイル形式・主要な仕組みが変わったとき、要目視確認の項目が増えた/片付いたときは **status.md** の該当節を現状に書き換える(古い記述は併記せず置き換える)
- 経緯として残したいこと(何を・なぜ変えたか、その時点の要目視確認)は **changelog.md** の先頭にエントリを足す
- `SimParams` のパラメータや顔料を追加・変更したら **parameters.md** を更新する
- パス順序・テクスチャ構成・モジュール構成・シェーダーが変わったら **architecture.md** を更新する
- 任意の負債 R6〜R8 に手を付けたら **refactoring.md** の状態列を更新する

## コマンド

Rust は mise で管理(`mise.toml`、rust 1.96.0)。ビルド・実行は `mise exec -- cargo run` / `cargo test --workspace` / `cargo clippy --workspace`。**workspace 構成**(R1: km / pigment / paint-core の 3 crate + ルートの kasaneiro crate(lib + 起動だけの bin)。ルートが同時に package なので、下位 crate のテストまで回すには `--workspace` を付ける)。

- コンパイル時間対策として `[profile.dev.package."*"] opt-level = 2` + 自前コード opt-level 1 を設定する方針(plan.md §2。profile はワークスペースルート Cargo.toml でのみ有効)
- `crates/km`(Kubelka-Munk 純関数)と `crates/pigment`(mixbox 混色)は CPU 参照実装 + `cargo test` の対象。WGSL は naga のコンパイル可能性テスト([tests/shader_compile.rs](tests/shader_compile.rs))で守る
- **e2e**(GPU 必須): [tests/e2e_watercolor.rs](tests/e2e_watercolor.rs) = headless でストローク再生 → 3要件を色相・明度の帯で判定 / [tests/e2e_ui.rs](tests/e2e_ui.rs) = egui_kittest で PaintApp を実入力で操作。流体シェーダーの数値は固定せず「体験が壊れていないか」だけを見る回帰検知なので、調整で指標が外れたら `target/tmp/e2e/*.png` を見て、体験として正しければ閾値側を直す。UI 要素は AccessKit の名前で引くので、文字のないウィジェットを足すときは `widget_info` で名前を付ける

## 設計の核心原則: 試行錯誤の速度を最優先

品質はアルゴリズム選択ではなく**パラメータ調整の反復回数**で決まる。これを支える構造を壊さないこと:

- **WGSL はビルドに埋め込まず `assets/shaders/` から実行時ロード**(ホットリロード H1)。シェーダーのコンパイルエラーでクラッシュさせず、オーバーレイ表示して直前の正常なパイプラインで続行する
- 全シミュレーションパラメータは単一の `SimParams` 構造体に集約し uniform buffer 化。**パラメータ追加 = フィールド1行 + スライダー1行 + WGSL 1行**の定型作業を維持する(手順は parameters.md §10)
- `assets/presets/*.json`(SimParams プリセット)・`assets/palettes/*.json`(パレット=4色一式)・`assets/pigments/*.json`(色単体ライブラリ)・`assets/strokes/*.json`(テストストローク)は **git にコミットする**
- マイルストーンの完了条件は数値ではなく「目で見て判定できる体験」(例: 黄+青を隣接させて緑に馴染むか)

## 先送り済みの判断(再検討トリガーが来るまで蒸し返さない)

plan.md §4 の表が正典。要点: 混色は mixbox クレート(CC BY-NC、商用化時に自作スペクトラル WGM へ — **mixbox 依存は `crates/pigment` に隔離**(R1 で依存グラフとして強制))/ 流体は Curtis 簡略版(不安定なら Stam、表情不足なら LBM)/ 同時顔料数は 4ch のまま(多色化はレイヤーごとパレット記録 M5c で)/ ペン入力は Windows Ink のみ(WinTab 非対応)/ Web 版はやらないが input・保存の trait 抽象だけ維持。
