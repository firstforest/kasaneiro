# かさねいろ(kasaneiro)技術調査 概要

アナログ水彩の経験に基づく「欲しい機能」を実現するための要素技術調査のまとめ。調査日: 2026-07-03。最新の研究・論文・商用実装・OSSをWeb調査し、要素技術ごとにノートを分けている。

**実装言語は Rust に決定**(2026-07-03)。実現可能性の評価と Rust 版の推奨構成は [05-rust-feasibility.md](05-rust-feasibility.md) を参照。シミュレーション核心(WGSL compute + KM混色)は言語選択と独立に成立するため、01〜03 のノートは変更なくそのまま適用できる。

## 欲しい機能と対応する要素技術

### 1. 先に置いた色の近くに別の色で描くと、馴染んで綺麗なグラデーションになる

**= ウェットオンウェット。2つの技術の組み合わせで実現する。**

- **水と顔料の流体シミュレーション** → [01-fluid-simulation.md](01-fluid-simulation.md)
  - 紙の上の水の流れ(浅水方程式 or 格子ボルツマン法)が顔料を移流・拡散させることで、色が「勝手に」馴染む。Curtis 1997 が古典で、GPU化すれば現代ではリアルタイムに動く
- **物理的に正しい混色** → [02-pigment-mixing.md](02-pigment-mixing.md)
  - 馴染んだ色がRGB補間だと灰色に濁る。Kubelka-Munk理論ベースの混色(Mixbox または spectral.js)で「黄+青=緑」になる

### 2. 一度乾いた色の上に描くと、綺麗に重なった描画になる(レイヤー構造でいい)

**= グレージング(wet-on-dry)。** → [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md)

- Curtis 1997 のモデルがまさに「**レイヤー確定=乾燥**」という構造(glaze列)で、レイヤー構造でいいという要件と完全に一致する
- 層の重なりは Kubelka-Munk の反射率/透過率合成式で光学的に合成すると「内側から光る」水彩らしい重なりになる。式は単純でシェーダー1パスで書ける
- 段階的実装: まず multiply ブレンド → 後から KM 合成に置き換え可能

### 3. 既に描いた色を削って白い部分を作れる(デジタルならでは)

**= リフティング+完全消去の2ツール構成を推奨。** → [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md) §5

- 物理的なリフティング(Curtis の吸着/脱着モデル): ステイニングの床が残る・縁が柔らかい・剥がした顔料が縁に再沈着する、という水彩らしい削り
- デジタルならではの完全消去(紙の白まで戻す)は別ツールとして提供(Corel Painter と同じ分離)

## ノート一覧

| ファイル | 内容 |
|---|---|
| [01-fluid-simulation.md](01-fluid-simulation.md) | にじみ・ウェットオンウェットの流体シミュレーション(Curtis 1997 / MoXi LBM / リアルタイム化 / 商用ソフト / 紙モデル / エッジダークニング / 乾燥) |
| [02-pigment-mixing.md](02-pigment-mixing.md) | 物理的に正しい混色(Kubelka-Munk / Mixbox / spectral.js / 2022年以降の動向) |
| [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md) | 重ね塗り(KM層合成・グレージング)と削り(リフティング・粒状感・ステイニング) |
| [04-implementation-stack.md](04-implementation-stack.md) | 実装技術スタック(WebGPU/WebGL2/wgpu比較、参考OSS、入力処理、性能目安、推奨構成) |
| [05-rust-feasibility.md](05-rust-feasibility.md) | Rustでの実現可能性評価(wgpu / winit 0.31ペン入力 / octotablet / egui / mixboxクレート、Rust版推奨構成) |

## 推奨アーキテクチャ(調査結果の結論)

```
┌─ UI (egui: カラーピッカー・レイヤーパネル・調整スライダー) ────┐
│                                                              │
│  入力: winit 0.31 Pointer + octotablet (筆圧・傾き)           │
│    └→ ブラシスタンプ位置に水+顔料をsplat                     │
│                                                              │
│  シミュレーション (WGSL compute, 512²から開始, ping-pong):    │
│    テクスチャ: 水量+速度uv / 浮遊顔料 / 沈着顔料 / 紙ハイト     │
│    毎フレーム: 移流 → 発散緩和 → FlowOutward(縁の暗まり)      │
│               → 顔料移流 → 吸着/脱着 → 蒸発                  │
│    ※ アクティブ(濡れている)タイルのみ更新                     │
│                                                              │
│  レイヤー: アクティブな湿レイヤーは1枚だけシミュレーション        │
│    「乾かす」操作でRGBAレイヤーに焼き込み(=グレーズ確定)       │
│    レイヤー合成は multiply → 後日KM合成へ                     │
│                                                              │
│  混色: mixboxクレート(非商用CC BY-NC) or 自作スペクトラルWGM   │
│  削り: リフティングツール(ステイン床あり) + 完全消去ツール      │
└──────────────────────────────────────────────────────────────┘
プラットフォーム: Rust + wgpu + winit + egui、配布は単一exe
(将来 wasm32 + WebGPU で Web 版の道あり → 05-rust-feasibility.md)
```

> **追記(2026-07-05)**: 図中の octotablet は M1.5 実装時に**不採用**になった(Windows でメッセージループがデッドロックする既知バグ)。筆圧は winit/egui 標準の Touch イベントで取得している — [05-rust-feasibility.md](05-rust-feasibility.md) §2 の追記参照。

顔料ごとのパラメータは **密度ρ(沈着速度)/ ステイニングω(剥がれにくさ)/ 粒状感γ(紙目への反応)+ K,S(色)** の組で、水彩絵具の個性(粒状化するウルトラマリン、ステイニングするフタロ等)がほぼ全て表現できる。Curtis 1997 論文に12種の顔料の実値表があり流用可能。

## 実装ロードマップ

0. **Phase 0**(Rust化で追加): winit + wgpu + egui の空アプリ(ウィンドウ+テクスチャ表示+スライダー+WGSLホットリロード)→ [05](05-rust-feasibility.md) §8
1. **Phase 1**: Curtis簡略版の流体シミュ(512²)+ Mixbox混色 + splatブラシ → wet-on-wetの「馴染むグラデーション」が体感できる最小構成(ブラシはマウスで開始、筆圧は後半にoctotabletで追加)
2. **Phase 2**: 「乾かす」ボタン+レイヤー焼き込み(グレージング)、アクティブタイル最適化
3. **Phase 3**: リフティング(削り)ツール、粒状感・ステイニングの顔料パラメータ、KM層合成
4. **Phase 4(任意)**: エッジダークニングの調整、紙テクスチャの差し替え、LBM換装や超解像などの高度化
