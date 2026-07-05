# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

**my-paint** — Rust + wgpu 製の水彩シミュレーションペイントツール(Windows 個人用ツール)。3つのコア要件 = **wet-on-wet**(色が馴染むグラデーション)/ **グレージング**(乾いた色の上に綺麗に重なる)/ **削り**(リフト+完全消去の2ツール)。ドキュメント・コミットメッセージは日本語。

## ドキュメントの役割(作業開始時にまず status.md を確認する)

| ドキュメント | 役割 |
|---|---|
| [docs/status.md](docs/status.md) | **現在の実装状況の正典**。作業開始時にまず読む |
| [docs/requirements.md](docs/requirements.md) | 要件仕様(3要件の判定基準・ツール構成・UI/入力要件・非機能要件) |
| [docs/architecture.md](docs/architecture.md) | 実装構造(スタック・モジュール構成・テクスチャ・パス順序・シェーダー一覧・混色の2段構え) |
| [docs/parameters.md](docs/parameters.md) | 全パラメータ(`SimParams`)と顔料個性 ρ/ω/γ のリファレンス(既定値・UI 範囲・意味・実効式) |
| [docs/plan.md](docs/plan.md) | 実装計画(マイルストーン M0〜M8・装備 H1〜H6・先送り判断 §4) |
| [docs/refactoring.md](docs/refactoring.md) | リファクタリング計画(R1〜R9。各項目をどのマイルストーン前に適用するか・やらないことの表) |
| [docs/note/00-overview.md](docs/note/00-overview.md) | 技術調査5ノートの起点。設計判断に迷ったら参照 |

**更新規則(コード変更と同じコミットで):**

- マイルストーン・装備(H1〜H6)を進めたら **status.md** を更新する
- `SimParams` のパラメータや顔料を追加・変更したら **parameters.md** を更新する
- パス順序・テクスチャ構成・モジュール構成・シェーダーが変わったら **architecture.md** を更新する
- リファクタリング項目(R1〜R9)を適用したら **refactoring.md** の状態列を更新する

## コマンド

Rust は mise で管理(`mise.toml`、rust 1.96.0)。ビルド・実行は `mise exec -- cargo run` / `cargo test` / `cargo clippy`。

- コンパイル時間対策として `[profile.dev.package."*"] opt-level = 2` + 自前コード opt-level 1 を設定する方針(plan.md §2)
- `km.rs`(Kubelka-Munk 純関数)と mixbox 混色は CPU 参照実装 + `cargo test` の対象。流体シェーダー本体は挙動をテストせずデバッグ表示(H4)で診断する方針。WGSL は naga のコンパイル可能性テスト([tests/shader_compile.rs](tests/shader_compile.rs))のみ守る

## 設計の核心原則: 試行錯誤の速度を最優先

品質はアルゴリズム選択ではなく**パラメータ調整の反復回数**で決まる。これを支える構造を壊さないこと:

- **WGSL はビルドに埋め込まず `assets/shaders/` から実行時ロード**(ホットリロード H1)。シェーダーのコンパイルエラーでクラッシュさせず、オーバーレイ表示して直前の正常なパイプラインで続行する
- 全シミュレーションパラメータは単一の `SimParams` 構造体に集約し uniform buffer 化。**パラメータ追加 = フィールド1行 + スライダー1行 + WGSL 1行**の定型作業を維持する(手順は parameters.md §10)
- `assets/presets/*.json`(SimParams・顔料プリセット)と `assets/strokes/*.json`(テストストローク)は **git にコミットする**
- マイルストーンの完了条件は数値ではなく「目で見て判定できる体験」(例: 黄+青を隣接させて緑に馴染むか)

## 先送り済みの判断(再検討トリガーが来るまで蒸し返さない)

plan.md §4 の表が正典。要点: 混色は mixbox クレート(CC BY-NC、商用化時に自作スペクトラル WGM へ — **mixbox 呼び出しは pigment.rs に隔離**)/ 流体は Curtis 簡略版(不安定なら Stam、表情不足なら LBM)/ 同時顔料数は 4ch のまま(多色化はレイヤーごとパレット記録 M5c で)/ ペン入力は Windows Ink のみ(WinTab 非対応)/ Web 版はやらないが input・保存の trait 抽象だけ維持。
