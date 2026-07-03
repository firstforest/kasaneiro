# 水彩のにじみ・ウェットオンウェットの流体シミュレーション

「濡れた紙の上で隣の色と馴染んで綺麗なグラデーションになる」現象をデジタルで再現するための技術ノート。調査日: 2026-07-03。

関連ノート: [02-pigment-mixing.md](02-pigment-mixing.md) / [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md) / [04-implementation-stack.md](04-implementation-stack.md)

---

## 全体像(手法の系統)

水彩シミュレーションは大きく2系統に分かれる。

1. **物理シミュレーション系**: キャンバスをグリッドで表し、水と顔料の移流・拡散を毎フレーム解く。**浅水方程式(Curtis 1997系)** と **格子ボルツマン法(MoXi 2005系)** が二大流派。にじみ・バックラン(戻りにじみ)・エッジダークニングが物理から「創発」する。
2. **見た目再現(NPR)系**: 流体は解かず、画像処理・シェーダーでエッジダークニングや粒状感を後付けする(Bousseau 2006、Montesdeoca 2017系)。安価で高速だが、「描いた後も絵具が動き、隣の色と馴染み続ける」体験は得られない。

商用ツール(Rebelle / Expresii / Fresco)はいずれも物理シミュレーション系のGPU実装。ML系は現状「ストロークの見た目の学習」が中心で、wet-on-wetの流体挙動を置き換える決定打はまだない(=研究ギャップ。古典物理シミュのGPU実装が依然として実用解)。

**「色が馴染むグラデーション」という要件には物理シミュレーション系が必須。** その上で仕上げにNPR系の安価なテクニックを併用できる。

---

## 1. Curtis et al. 1997 "Computer-Generated Watercolor"(古典・浅水方程式)

- Curtis, Anderson, Seims, Fleischer, Salesin, SIGGRAPH 1997
- [プロジェクトページ](https://grail.cs.washington.edu/projects/watercolor/) / [論文PDF](https://grail.cs.washington.edu/projects/watercolor/paper_small.pdf) / [わかりやすい解説(WPI講義ページ)](https://davis.wpi.edu/~matt/courses/watercolor/fluid.html)

**キャンバスを3層のセルグリッドでモデル化**:

1. **浅水層 (shallow-water layer)**: 紙の上を流れる水。簡略化した浅水方程式(スタガード格子)で速度場・水深を更新。この流れが顔料を移流させる — **wet-on-wetのにじみの本体**
2. **顔料沈着層 (pigment-deposition layer)**: 顔料が水中(浮遊)と紙面(沈着)を行き来する。→ [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md) §4
3. **毛細管層 (capillary layer)**: 紙繊維内の水分。毛細管輸送で濡れ領域がじわじわ広がり、バックラン(戻りにじみ)の元になる。**バックランが不要なら省略できる**

**メインループ**(毎タイムステップ、全サブルーチンの擬似コードが論文に完全公開されている):

```
MoveWater(M, u, v, p)       // 速度更新 → 発散の反復緩和 → FlowOutward(エッジダークニング)
MovePigment(M, u, v, g_k)   // 顔料の移流(風上差分的セルオートマトン)
TransferPigment(g_k, d_k)   // 吸着・脱着(→ 03 §4)
SimulateCapillaryFlow(M, s) // バックラン用(省略可)
```

- 発散除去はPoissonソルバ不要の**反復緩和**(δ = −ξ·div、ξ=0.1、最大50回)で、実装が非常に単純
- 時間積分は適応ステップの前進オイラー(速度が1セル/ステップを超えないよう制限)
- 論文に**12種の顔料パラメータ表(K, S, ρ, ω, γ)が全部載っており、そのまま流用可能**

**計算コスト**: 1997年当時は 640×480・11グレーズで133MHz SGI上7時間のオフライン処理。ただし全セル並列のグリッド更新なので現代のGPU compute shaderに素直に載り、1024²〜2Kで60fpsが現実的([TAMU修士論文にGPU化の先例](https://oaktrust.library.tamu.edu/server/api/core/bitstreams/5575e4a6-40fd-4946-ad32-f83712ddc02f/content))。

**採用**: ★推奨。「まずCurtisをcompute shaderで再実装する」のが水彩シミュ入門の定石。水量・速度u/v・浮遊顔料・沈着顔料・紙ハイトの5〜9枚のテクスチャをping-pong更新するだけで骨格が組める。注意点は (a) 解像度依存(論文自身が認める)、(b) パラメータ調整が見た目を大きく左右すること。

## 2. Chu & Tai 2005 "MoXi"(格子ボルツマン法・リアルタイムの嚆矢)

- *MoXi: Real-Time Ink Dispersion in Absorbent Paper*, ACM TOG 24(3), SIGGRAPH 2005
- [SIGGRAPH History](https://history.siggraph.org/learning/moxi-real-time-ink-dispersion-in-absorbent-paper-by-chu-and-tai/) / [ACM DL](https://dl.acm.org/doi/10.1145/1073204.1073221) / 著者版PDF: `http://visgraph.cse.ust.hk/MoXi/moxi.pdf`(**httpsだと404になるので必ず http:// でアクセス**)

**核となるアイデア**: 水の流れを**格子ボルツマン方程式 (LBE, D2Q9)** で解く。各セルに9方向の粒子分布関数を持たせ、「衝突(平衡分布への緩和)→伝搬(隣セルへ移動)」の局所計算だけで流体を更新する。

- Curtisのモデルは wet-dry 境界が固定で形状が自発進化しない。MoXiは**自由境界+ピン止め**により、にじみ境界の複雑なフラクタル状パターンが自発的に進化する
- 紙の繊維構造はセルごとの**ブロッキング係数 κ**(部分バウンスバック)として組み込む。κ を紙のスキャンテクスチャ等で変調すると枝分かれ模様が出る
- Poissonソルバ不要・全操作が局所的なのでGPUと極めて相性が良い。1ステップ=12回のテクスチャ更新、シェーダーの平均命令数はわずか約30
- **実測性能(2005年, GeForce 6800 Ultra)**: 256²で70fps、512²で48fps、512²+3倍アップサンプル描画込みで44fps。現代GPUなら4Kでも余裕
- エッジダークニングは「ピン止め境界での追加蒸発」で再現(Curtisの FlowOutward の空間フォールオフ版)

**採用**: 難易度は中〜やや高。LBE本体(streaming+collision)は浅水方程式より書きやすいが、自由境界・ピン止め・移流変調などの拡張群とテクスチャ群の作り込みが品質の本体。**東洋的な墨のにじみ・自由境界の形状進化**を出したいならMoXi系、西洋水彩のウォッシュ・グレーズ中心ならCurtis系のほうが制御しやすい。MoXiはKM合成を持たない(Future Work扱い)ので、**LBE流体+CurtisのKM合成のハイブリッドが現代の個人開発では最も費用対効果が高い構成**。MoXiは2006年にAdobeにライセンスされ、著者自身の商用化がExpresii(後述)。

## 3. 浅水方程式系の発展(リアルタイム化)

### Van Laerhoven & Van Reeth 2005 "Real-time simulation of watery paint"

- Computer Animation and Virtual Worlds 16(3-4), 2005 — [Wiley](https://onlinelibrary.wiley.com/doi/abs/10.1002/cav.95)(前身: CGI 2004、[PDF](http://www.cs.ucf.edu/courses/cap6105/fall09/readings/watercolor_sim.pdf))

Curtisの3層モデルをリアルタイム最優先に再設計。重要な変更点が2つ:

1. 浅水層のソルバを Foster-Metaxas 式から **Stam の安定流体法(セミラグランジアン)に置換**(「Foster-Metaxasは遅く高粘性で不安定」が動機)。「Curtisの各ステップをStam流に安定化する」という発想は個人実装への最有力ヒント
2. キャンバスをサブペーパー群に分割し**アクティブな領域だけシミュレーション**。当時はMPIクラスタ分散だったが、今日ではGPU1枚+アクティブタイル方式に読み替えられる。このタイル方式は**個人開発で最も費用対効果の高い最適化**であり、Adobe Frescoも同種のタイルベース処理を採る

水彩・ガッシュ・墨まで対応。当時の性能は6CPUで256²を25〜30fps。

### Stuyck et al. 2017 "Real-Time Oil Painting on Mobile Hardware"

- Computer Graphics Forum 36(8), 2017 — [プロジェクトページ](https://graphics.cs.kuleuven.be/publications/SD2016RTOPOMH/index.html) / [著者版PDF](https://tuurstuyck.github.io/assets/oilpaint_low_res.pdf)

油彩向けだが★実装ヒントの宝庫。**浅水方程式をモバイルGPU向けに徹底的に簡略化**する方法(どの項を削ってよいか、粘性・重力・キャンバス傾きの入れ方)が具体的に書かれており、水彩版に読み替えやすい。

### その他の近年の研究

- **Chen et al. 2015 "Wetbrush"**(SIGGRAPH Asia 2015, [ACM](https://dl.acm.org/doi/10.1145/2816795.2818066), [NVIDIA blog](https://developer.nvidia.com/blog/gpu-based-3d-painting-simulation/)) — 筆の毛1本1本と3D流体をCUDAで一体シミュレーション。Adobe Frescoの油彩Live Brushの源流
- **Canabal et al. 2020 "Simulation of Dendritic Painting"**(Eurographics 2020, [Wiley](https://onlinelibrary.wiley.com/doi/10.1111/cgf.13955)) — 表面張力・粘性フィンガリングによる樹枝状のにじみの物理
- 2020年以降、インタラクティブ水彩流体の純粋な学術論文は少なく、**イノベーションの主戦場は商用ソフトに移っている**

## 4. 機械学習ベースの手法(現状の正直な評価)

- **Shugrina et al. 2022 "Neural Brushstroke Engine"**(NVIDIA, SIGGRAPH Asia 2022, [プロジェクト](https://research.nvidia.com/labs/toronto-ai/brushstroke_engine/), [GitHub](https://github.com/nv-tlabs/brushstroke_engine)) — 約200枚のストローク画像からGANが画材の見た目を学習しリアルタイム描画。**ただし流体状態を持たない**ため、描いた後に絵具が動き続ける・隣の色と馴染み続ける本物のwet-on-wet挙動は原理的に出ない
- **Differentiable painting系**(Learning to Paint [arXiv:1903.04411](https://arxiv.org/abs/1903.04411)、Stylized Neural Painting [arXiv:2011.08114](https://arxiv.org/abs/2011.08114) など)は「画像を再現するストローク列の最適化」であり、手描きツールのwet-on-wetには寄与しない
- 2026年時点で「wet-on-wetのにじみをNNで置き換えて実用化した」例は確認できず。**MLの使いどころはブラシ見た目の多様化と超解像(Rebelle NanoPixel方式)に限定するのが妥当**

## 5. 商用ソフトの実装アプローチ

| 製品 | 技術 |
|---|---|
| **Rebelle** ([about](https://www.escapemotions.com/products/rebelle/about), [SIGGRAPH 2016 Talk](https://dl.acm.org/doi/10.1145/2936744.2936747)) | Navier-Stokes系CFDによるリアルタイム水分・顔料流体シミュレーション。キャンバス傾きによる流れ、Blowツール、Wet/Dryツール(部分的に濡らす/乾かす)。**DropEngine**(垂れ・ドリップ専用の別エンジン — 汎用CFD一本ではなく用途別分割が実用的)。色混合はKM系(Mixbox搭載)。**NanoPixel**は流体技術ではなくML超解像(シミュは作業解像度、表示は16倍でエクスポート)([blog](https://www.escapemotions.com/blog/rebelle-5-nanopixel-export-high-res-canvases-thanks-to-machine-learning)) |
| **Expresii** ([公式](https://www.expresii.com/), [Moxi Paint Engine](https://www.expresii.com/moxi-paint-engine.html)) | MoXi著者 Nelson Chu 本人による商用化。Moxi Paint Engine(GPU水彩流体、LBM系譜)+ Yibi Brush Engine(3D筆モデル)+ Youji Rendering Engine(vector/rasterハイブリッドで100倍ズームでも劣化しない)。**LBM論文がそのまま製品になった実例**。GT 730程度のローエンドGPUでも動作 |
| **Adobe Fresco** ([Live brushes](https://helpx.adobe.com/fresco/using/live-brushes.html), [Project Wetbrush](https://research.adobe.com/news/adobe-nvidia-wet-brush/)) | Wetbrush著者らによる物理シミュレーションエンジン(詳細非公開)。水彩はタイルベース処理でiPadでもリアルタイム。**水彩は「永遠に乾かない」**(乾燥モデルを持たない割り切り)。露出パラメータを water flow / color flow 程度まで絞る製品設計 |
| **Corel Painter** ([Real Watercolor controls](http://product.corel.com/help/Painter/540215550/Main/EN/Win-Documentation/Corel-Painter-Real-Watercolor-controls.html)) | 専用Watercolorレイヤーに描画し「wet」にすると拡散が起動。パラメータ設計が参考になる: Wetness / Concentration / Viscosity / Evaporation Rate / Flow Resistance(紙目の抵抗)/ Settling Rate / Weight / Pickup(再溶解)/ Wind |
| **Krita** ([議論](https://krita-artists.org/t/real-fluid-brush-engines-such-as-real-watercolor/26155)) | 物理ベースの流体水彩エンジンは**現状存在しない**(Color Smudge+プリセットで水彩「風」)。GSoCでCurtis系のsplat+wet map方式の試作あり(本体未マージ)。OSSでも本格CFD水彩が定着していないのは性能とUX調整の難度の証左 |

## 6. 紙のモデル化(毛細管現象・紙目)

- **Curtis 1997**: 紙はハイトフィールド h∈[0,1](Perlinノイズ+繊維状ストリークの合成)。h が3箇所に効く:
  1. **流体**: 速度更新時に高さ勾配 ∇h を圧力勾配に加算(水は谷へ流れる)→ 紙目に沿ったストリーク
  2. **保水容量**: `c = h·(c_max − c_min) + c_min`
  3. **顔料の吸着/脱着率**: 凹部に顔料が溜まる → 粒状感
- **実装ヒント**: オクターブ違いのPerlinノイズ2枚(低周波=沈殿ムラ、高周波=紙目)を作り、上記3箇所に同じハイトマップを参照させるだけで一貫した紙の存在感が出る。実紙スキャンのハイトマップへの差し替えも容易(Rebelleは実紙マイクロスキャンを使用)
- 紙内拡散の源流研究: Guo & Kunii 1991(墨の拡散、粒子サイズ依存の吸着+液体流)、Way, Huang & Shih 2003(繊維メッシュモデル)。フルの繊維幾何を持たなくても「容量マップ+異方性拡散重み(繊維方向)」の2テクスチャで同等の見た目が得られる

## 7. エッジダークニング(縁に顔料が溜まる現象)

- **物理的根拠**: Deegan et al. "Capillary flow as the cause of ring stains from dried liquid drops", **Nature 389, 827–829 (1997)** ([nature.com](https://www.nature.com/articles/39827))。コーヒーリング効果 — ①接触線のピン留め ②縁での蒸発フラックス大 ③補償のため内部→縁への毛細管流 ④粒子が縁に堆積
- **物理的に出す(Curtis 1997)**: **FlowOutward** ステップ。濡れ領域マスク M をガウシアンぼかしした M' を使い、境界に近いセルほど水を多く除去する `p ← p − η·(1−M')·M`。シミュレーションしていれば追加コストほぼゼロで縁が創発する
- **画像処理で偽装する**:
  - Bousseau et al. 2006 (NPAR, [プロジェクト](https://artis.inrialpes.fr/Publications/2006/BKTS06/)): 顔料密度の変調式 `C' = C·(1 − (1−T)·(d−1))` にエッジ検出値・Perlinノイズ・紙テクスチャを流し込むだけで、エッジダークニング/濁り/粒状感を一括表現
  - Montesdeoca et al. 2017 "MNPR" ([artineering.io](https://artineering.io/publications/art-directed-watercolor-stylization-of-3d-animations-in-real-time)): DoG(Difference of Gaussians)による特徴強調+段階的な顔料蓄積。リアルタイム3D向けのオープンソースフレームワーク。**安価な偽装テクニックのカタログとして最良**
- **最安レシピ**: ストロークのアルファから距離場/DoGで「縁バンド」を作り、バンド内を濃度乗算(`rgb^(1+k·band)` のガンマ持ち上げが「同色相で濃くなる」水彩の縁に近い)

## 8. 乾燥プロセスのモデル化

| 方式 | 内容 | 採用例 |
|---|---|---|
| 蒸発+吸収による自然乾燥 | 各セルの水量を毎ステップ蒸発率で減衰、紙(毛細管層)へも吸収。水が無くなったセルで浮遊顔料を沈着層へ固定 | Curtis 1997(標準形) |
| 湿り気マップ | セルごとに wet/damp/dry を持ち、新ストロークの混ざり方(にじむ/エッジが立つ)を切替。バックランは「乾きかけ領域への水の侵入」で発生 | Curtis の毛細管層、Van Laerhoven 2005 |
| ユーザー制御乾燥 | Dry/Wetツール、レイヤー一括乾燥ボタン、送風 | Rebelle |
| 永続ウェット | 乾燥モデルなし。実装が単純でいつでも混色可能 | Adobe Fresco |

**実装パターン(統合)**:

```
wetness -= evaporation_rate + edge_bonus   // edge_bonus = blur(wetness)との差分 → 縁ほど速く乾く
移流・拡散の強度を wetness でスケール        // 乾くほど動かない
deposit_rate = base · (1 − wetness) · (1 + granulation · paper_height)
wetness == 0 で浮遊顔料を全量固定
```

- 浮遊顔料は彩度高め・明るめ、沈着顔料はKM風に濃く合成すると、「乾くと少し薄くなる」実物の挙動(dry shift)も表現できる
- 乾燥系パスは1/4解像度・数フレームに1回の更新で十分(視覚的にゆっくりなので)
- **ストローク堆積と拡散シミュレーションは別フェーズに分離する**(Rebelle の初期からの決定的最適化。拡散は低頻度ティックやストローク間に回せる)
- **UXの結論**: 自然乾燥をリアルタイムで待たせるより、**Rebelle式のユーザー制御乾燥(乾かすボタン、常に見えるワンタップ)**のほうが道具として使いやすい、というのが商用ソフトの収斂した答え。「レイヤー確定=乾燥」モデルや乾燥レイヤー管理の共通アーキテクチャは [03](03-layering-glazing-lifting.md) §6 に詳しい

## 9. 実装ロードマップ(推奨)

1. **Phase 1 — Curtis簡略版**: 水量・速度・浮遊顔料・沈着顔料・紙ハイトの5テクスチャをcompute shaderでping-pong更新。蒸発+FlowOutwardでエッジダークニングまで出す。Stuyck 2017の簡略化テクニックを参照。512²〜1024²@60fpsから開始
2. **Phase 2 — アクティブタイル最適化**: 濡れているタイルだけ更新して全画面常時シミュを回避(Van Laerhoven / Fresco方式)
3. **Phase 3 — 選択的高度化**: 繊維にじみが欲しければLBM(MoXi)へ換装、色のリアリティはKM混色([02](02-pigment-mixing.md))、拡大品質は超解像
