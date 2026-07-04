# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

**my-paint** — Rust + wgpu 製の水彩シミュレーションペイントツール(Windows 個人用ツール)。3つの要件を実現する:

1. **wet-on-wet**: 先に置いた色の近くに描くと馴染んで綺麗なグラデーションになる(流体シミュレーション + 物理混色)
2. **グレージング**: 乾いた色の上に描くと綺麗に重なる(レイヤー構造、乾燥 = レイヤー焼き込み)
3. **削り**: リフティングツール(ステイン床あり)+ 完全消去ツールの2ツール構成

実装計画は [docs/plan.md](docs/plan.md)、**現在の実装状況は [docs/status.md](docs/status.md) が正典**(作業開始時にまず確認する)。技術調査は [docs/note/00-overview.md](docs/note/00-overview.md) 起点の5ノート。設計判断に迷ったらまずこれらを参照すること。ドキュメント・コミットメッセージは日本語。

**マイルストーン・装備(H1〜H6)を進めたら docs/status.md を同じコミットで更新する。**

## コマンド

Rust は mise で管理(`mise.toml`、rust 1.96.0)。ビルド・実行は `mise exec -- cargo run` / `cargo test` / `cargo clippy`。

- コンパイル時間対策として `[profile.dev.package."*"] opt-level = 2` + 自前コード opt-level 1 を設定する方針(plan.md §2)
- `km.rs`(Kubelka-Munk 純関数)は CPU 参照実装 + `cargo test` の対象。流体シェーダー本体はテストせずデバッグ表示で診断する方針

## アーキテクチャ(決定事項)

- **スタック**: wgpu(WGSL compute)+ winit 0.31+ + egui/eframe/egui-wgpu + mixbox + octotablet(筆圧、M1.5から)+ notify(ホットリロード)。**egui の対応バージョンを起点に wgpu/winit をロックステップで固定**する
- **シミュレーション**: Curtis 1997 簡略版。テクスチャ4枚(水量+速度 / 浮遊顔料 / 沈着顔料 / 紙ハイト)、512²、ping-pong。毎フレーム: 移流 → 発散緩和 → FlowOutward → 顔料移流 → 吸着/脱着 → 蒸発
- **レイヤー**: 湿レイヤーは常に1枚。「乾かす」ボタンで RGBA レイヤーに焼き込み。合成は multiply で開始し M3 で KM 合成へ
- **単一クレート**で開始。プロジェクト構成(src/gpu, src/sim, brush.rs, input.rs, replay.rs, km.rs)は plan.md §2 参照

## 設計の核心原則: 試行錯誤の速度を最優先

品質はアルゴリズム選択ではなく**パラメータ調整の反復回数**で決まる。これを支える構造を壊さないこと:

- **WGSL はビルドに埋め込まず `assets/shaders/` から実行時ロード**(ホットリロード H1)。シェーダーのコンパイルエラーでクラッシュさせず、オーバーレイ表示して直前の正常なパイプラインで続行する
- 全シミュレーションパラメータは単一の `SimParams` 構造体に集約し uniform buffer 化。**パラメータ追加 = フィールド1行 + スライダー1行 + WGSL 1行**の定型作業を維持する
- `assets/presets/*.json`(SimParams・顔料プリセット)と `assets/strokes/*.json`(テストストローク)は **git にコミットする**
- マイルストーンの完了条件は数値ではなく「目で見て判定できる体験」(例: 黄+青を隣接させて緑に馴染むか)

## 先送り済みの判断(再検討トリガーが来るまで蒸し返さない)

plan.md §4 の表が正典。要点: 混色は mixbox クレート(CC BY-NC、商用化時に自作スペクトラル WGM へ — **混色呼び出しは1関数に隔離**)/ 流体は Curtis 簡略版(不安定なら Stam、表情不足なら LBM)/ アクティブタイル最適化はやらない(512² で 60fps を割ったら)/ ペン入力は Windows Ink のみ(WinTab 非対応)/ Web 版はやらないが input・保存の trait 抽象だけ維持。
