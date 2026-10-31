# かさねいろ(kasaneiro)技術ノート 概要

水彩シミュレーションに関わる要素技術の調査と、実装で確かめた仕組みのノート。**再利用できる知見と最新の情報だけ**を置き、書かれた内容が古くなったら本文を書き換える。

時系列が意味を持つ記録(当時の検討・比較・試行錯誤の経緯)は [../memo/](../memo/) に `YYYY-MM-DD-` を頭につけて置く。実装の現在地は [status.md](../status.md)、構造は [architecture.md](../architecture.md)、パラメータの既定値は [parameters.md](../parameters.md) が正典。

## 欲しい機能と対応する要素技術

### 1. 先に置いた色の近くに別の色で描くと、馴染んで綺麗なグラデーションになる

**= ウェットオンウェット。2つの技術の組み合わせで実現する。**

- **水と顔料の流体シミュレーション** → [01-fluid-simulation.md](01-fluid-simulation.md)、本実装の水の広がり方は [05-wet-spreading.md](05-wet-spreading.md)
  - 紙の上の水の流れ(浅水方程式 or 格子ボルツマン法)が顔料を移流・拡散させることで、色が「勝手に」馴染む。Curtis 1997 が古典で、GPU化すれば現代ではリアルタイムに動く
- **物理的に正しい混色** → [02-pigment-mixing.md](02-pigment-mixing.md)
  - 馴染んだ色がRGB補間だと灰色に濁る。Kubelka-Munk理論ベースの混色(Mixbox または spectral.js)で「黄+青=緑」になる

### 2. 一度乾いた色の上に描くと、綺麗に重なった描画になる(レイヤー構造でいい)

**= グレージング(wet-on-dry)。** → [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md)

- Curtis 1997 のモデルがまさに「**レイヤー確定=乾燥**」という構造(glaze列)で、レイヤー構造でいいという要件と一致する
- 層の重なりは Kubelka-Munk の反射率/透過率合成式で光学的に合成すると「内側から光る」水彩らしい重なりになる。式は単純でシェーダー1パスで書ける

### 3. 既に描いた色を削って白い部分を作れる(デジタルならでは)

**= リフティング+完全消去の2ツール構成。** → [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md) §5

- 物理的なリフティング(Curtis の吸着/脱着モデル): ステイニングの床が残る・縁が柔らかい・剥がした顔料が縁に再沈着する、という水彩らしい削り
- デジタルならではの完全消去(紙の白まで戻す)は別ツールとして提供(Corel Painter と同じ分離)

顔料ごとのパラメータは **密度ρ(沈着速度)/ ステイニングω(剥がれにくさ)/ 粒状感γ(紙目への反応)+ 色** の組で、水彩絵具の個性(粒状化するウルトラマリン、ステイニングするフタロ等)がほぼ全て表現できる。Curtis 1997 論文に12種の顔料の実値表がある。

## ノート一覧

| ファイル | 内容 |
|---|---|
| [01-fluid-simulation.md](01-fluid-simulation.md) | にじみ・ウェットオンウェットの流体シミュレーション(Curtis 1997 / MoXi LBM / リアルタイム化 / 商用ソフト / 紙モデル / エッジダークニング / 乾燥) |
| [02-pigment-mixing.md](02-pigment-mixing.md) | 物理的に正しい混色(Kubelka-Munk / Mixbox / spectral.js / 2022年以降の動向)と、濃度→発色の調整の性質 |
| [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md) | 重ね塗り(KM層合成・グレージング)と削り(リフティング・粒状感・ステイニング) |
| [04-platform-and-references.md](04-platform-and-references.md) | 実装基盤(wgpu / egui / ペン入力 / mixbox クレート)の知見、参考実装・論文、性能の目安 |
| [05-wet-spreading.md](05-wet-spreading.md) | 本実装で置いた色が広がって馴染む仕組み(水の毛細管拡散・水依存の混ざり・しきい値型の吸着・筆の含み・溶かし戻し)と調整ガイド |
