# 物理的に正しい顔料混色(Kubelka-Munk / Mixbox / スペクトラル)

「別の色を取った筆で描くと色が馴染んで綺麗なグラデーションになる」ためには、流体シミュレーション([01](01-fluid-simulation.md))だけでなく、**混ざった色が絵具らしい色になる**混色モデルが必要。調査日: 2026-07-03。

関連ノート: [01-fluid-simulation.md](01-fluid-simulation.md) / [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md) / [04-implementation-stack.md](04-implementation-stack.md)

---

## 0. 背景: RGB線形補間の何が問題か

RGBは「光の加法混色」のモデルであり、`lerp(黄, 青, 0.5)` は彩度の低い灰色になる。実際の絵具は顔料による**減法混色+散乱**であり、黄+青=緑になる。この挙動を予測する物理モデルが Kubelka-Munk (K-M) 理論で、以下の技術はすべて「K-Mをいかに実用的な形でRGBワークフローに持ち込むか」の工夫。

## 1. Kubelka-Munk理論 (1931) — 基礎

原典: P. Kubelka, F. Munk, "Ein Beitrag zur Optik der Farbanstriche", *Zeitschrift für technische Physik* 12, pp. 593–601, 1931(フリー全文なし。数式の整理: [ScienceDirect Topics](https://www.sciencedirect.com/topics/engineering/kubelka-munk-theory))

### 核となる数式

**K/S(吸収/散乱比)と反射率の関係**(光学的に十分厚い層の反射率 R∞):

```
K/S = (1 − R∞)² / (2·R∞)
R∞  = 1 + (K/S) − √((K/S)² + 2(K/S))
```

**二定数混合則**(KとSを別々に、濃度 cᵢ に対して線形):

```
K_mix(λ) = Σᵢ cᵢ·Kᵢ(λ),   S_mix(λ) = Σᵢ cᵢ·Sᵢ(λ)
```

**単定数混合則**(染色業界由来の簡略版。K/S比のみ加法的と仮定):

```
(K/S)_mix = (K/S)_基材 + Σᵢ cᵢ·(K/S)ᵢ
```

**有限厚の層の反射率・透過率**(グレーズ重ね用。→ [03](03-layering-glazing-lifting.md) §3):

```
a = 1 + K/S,  b = √(a² − 1)
R = [1 − R_g(a − b·coth(bSX))] / [a − R_g + b·coth(bSX)]   (R_g: 背景反射率, X: 層厚)
T = b / [a·sinh(bSX) + b·cosh(bSX)]
```

**Saunderson補正 (1942)** — 空気/塗膜界面の表面反射の補正。実測反射率から顔料のK/Sを抽出する前処理として必須:

```
R_measured = k₁ + (1−k₁)(1−k₂)·R / (1 − k₂·R)     k₁ ≈ 0.04, k₂ ≈ 0.4–0.6
```

(J. L. Saunderson, "Calculation of the Color of Pigmented Plastics," *JOSA* 32(12), 1942)

### K-Mが2021年までペイントソフトに載らなかった理由

(a) 測定済みK/Sデータセットの入手難、(b) 波長ごとの非線形演算のコスト、(c) **任意のRGB色→顔料濃度への逆変換が不良設定問題**。この (c) を解いたのがMixbox。

### 古典的応用

| 研究 | 使い方 |
|---|---|
| Curtis et al. 1997 ([プロジェクト](https://grail.cs.washington.edu/projects/watercolor/)) | RGB3チャンネルのK・SでグレーズをK-M合成(最粗近似だが水彩には今でも実用的) |
| Baxter et al. "IMPaSTo" (NPAR 2004, [ACM](https://dl.acm.org/doi/10.1145/987657.987665), [PDF](http://gamma.cs.unc.edu/IMPASTO/publications/Baxter-IMPaSTo_Web-NPAR04.pdf)) | **8波長サンプル**のスペクトルK-MをGPU実装しリアルタイム化。ウェット層+乾燥層を層方程式で合成 |

## 2. Mixbox — Sochorová & Jamriška 2021(最重要)

"Practical Pigment Mixing for Digital Painting," *ACM TOG* (SIGGRAPH Asia 2021)
[ACM DL](https://dl.acm.org/doi/10.1145/3478513.3480549) / [論文PDF(無料)](https://dcgi.fel.cvut.cz/wp-content/wpallimport-dist/publications/pdf/publications-2021-sochorova-tog-pigments-paper.pdf) / [scrtwpns.com/mixbox](https://scrtwpns.com/mixbox/) / [GitHub](https://github.com/scrtwpns/mixbox)

### 核となるアイデア

1. **アンミキシング**: RGB色を実在の原色顔料4種(フタロブルー、キナクリドンマゼンタ、ハンザイエロー、チタニウムホワイト)の**濃度 c₁..c₄** に分解
2. **残差項**: 4顔料で表せない色との差分をRGB残差として保持 → 潜在空間は**7次元(濃度4+残差3)**。RGB→潜在→RGBの往復が**無損失**(混ぜなければ色が変わらない=既存アプリに安全に導入できる)
3. **混色**: 潜在空間で濃度を線形補間(K-Mの二定数混合則に相当)し、K-M+Saunderson補正でRGBに戻す
4. **高速化**: RGB→潜在の変換を**3D LUT**(`mixbox_lut.png`)に焼き込み。実行時はLUT参照+少量の多項式評価のみでシェーダーでも動く

APIは `mixbox_lerp(rgb1, rgb2, t)` の1関数で従来のlerpと差し替え可能。多色混合は `rgb_to_latent` → 潜在平均 → `latent_to_rgb`。

### ライセンス(重要)

- **非商用: CC BY-NC 4.0**(個人非商用は無料)。C/C++/C#/Java/JS/Python/Rust、**GLSL/HLSL/Metal**、Unity/Godot/WebGL対応
- **商用**: Secret Weapons社と個別契約(インディー向け条件あり。mixbox@scrtwpns.com)
- **GPLと非互換**: KritaへのMixbox統合はNCライセンスが障壁になり見送られた([krita-artists議論](https://krita-artists.org/t/can-we-get-mixbox-on-krita/64201))
- 採用実績: Rebelle 5 Pro以降の「Pigments」、Across the Spider-Verse の制作パイプライン([SIGGRAPH 2023 Talk](https://dl.acm.org/doi/10.1145/3587421.3595451))

**採用ヒント**: 無料配布の個人ツールならCC BY-NCの範囲でそのまま使えるのが最短ルート(難易度 ★☆☆ — LUT画像1枚+1ソースファイル)。有料販売・広告・寄付を予定するなら商用ライセンスか、次節の自作路線。

## 3. スペクトラル(波長ベース)混色 — ライセンスフリーの自作路線

RGB→分光反射率→(スペクトル空間で混合)→XYZ→RGBという経路。

### RGB→スペクトル復元(スペクトラルアップサンプリング)

| 論文 | 核アイデア |
|---|---|
| Smits 1999 ([DOI](https://www.tandfonline.com/doi/abs/10.1080/10867651.1999.10487511) / [実装](https://github.com/colour-science/smits1999)) | 白+RGB+CMYの7基底スペクトルの重み付き和。実装最容易 |
| Meng et al. 2015 (EGSR, [Wiley](https://onlinelibrary.wiley.com/doi/abs/10.1111/cgf.12676) / [PDF](https://jo.dreggn.org/home/2015_spectrum.pdf)) | XYZ全域で滑らかなスペクトルを事前最適化しLUT化 |
| Jakob & Hanika 2019 (Eurographics, [EPFL](https://rgl.epfl.ch/publications/Jakob2019Spectral)) | 反射スペクトルをsigmoid(2次多項式)の3係数で表現。1波長6 FLOPs。現代スペクトラルレンダラの標準 |

### 加重幾何平均 (WGM) — Scott Allen Burns

"Subtractive Color Mixture Computation" — [arXiv:1710.06364](https://arxiv.org/abs/1710.06364) / [解説サイト](http://scottburns.us/subtractive-color-mixture/)

反射率の**加重幾何平均**が単定数K-Mの良い近似になる:

```
R_mix(λ) = Πᵢ Rᵢ(λ)^cᵢ    (Σcᵢ = 1)
```

手順: sRGB→36サンプル反射率→WGM混合→CIE XYZ→sRGB。**K/S実測データ不要**が最大の利点。

### spectral.js — 個人開発の本命

[github.com/rvanwijnen/spectral.js](https://github.com/rvanwijnen/spectral.js)(v3.0.0, 2025年4月)— **MITライセンス**

- RGB→7基底の反射率カーブ(380–750nm、38サンプル)→濃度重み付きK/S混合→逆K-M→XYZ→sRGB。OKLab/ΔEでガマットマップ
- `spectral.mix('#002185', '#FCD200', 0.5)` → 緑 `#3D933E`。**「MixboxのライセンスフリーMIT代替」の事実上の答え**
- GLSL移植あり。Aseprite・Python(ColorAide)等への移植も存在
- **Krita統合の実例**: 作者とKrita開発者Dmitry Kazakovによる移植が進行中 — [議論スレッド](https://krita-artists.org/t/paint-like-color-mixing-kubelka-munk/78156)、[MR !1997](https://invent.kde.org/graphics/krita/-/merge_requests/1997)。sRGB往復誤差ゼロを確認、スマッジエンジン側での「緑化」アーティファクト等の**エンジニアリング上の落とし穴の記録として必読**

## 4. リアルタイム向けの近似手法(軽い順)

1. **LUT方式(Mixbox流)** — 3Dテクスチャフェッチ1–2回。GPUで最速 ★☆☆
2. **単定数K-M / WGM(Burns / spectral.js流)** — 38サンプルでもシェーダーで軽い(pow/exp程度)。散乱の強い不透明絵具では二定数より精度が落ちるが十分「絵具らしい」 ★★☆
3. **少数波長の二定数K-M(IMPaSTo流)** — 8波長でK・Sを持ち層方程式まで解く。層の厚みまで表現したい場合 ★★★
4. **RGB3チャンネルK-M(Curtis流)** — 最粗近似。水彩グレーズ合成には実用的 ★★☆
5. **チープハック** — ガンマ空間(≒RGB²)平均、RYB補間など。灰色化は緩和されるが黄+青=緑にはならない

検証済みGLSL実装: [STVND/davis-pigment-mixing](https://github.com/STVND/davis-pigment-mixing)(K-M混色、MIT)、Mixbox公式リポジトリ内シェーダー、[GitHubトピック kubelka-munk](https://github.com/topics/kubelka-munk)。

## 5. 2022年以降の動向

学術的には「絵具らしい混色」はMixboxで実用上解決とみなされ、フロンティアは流体・メディアシミュレーション側に移った。直接の後続論文は少ない:

- **Dripping Thin Films for Real-time Digital Painting**(Herson, Paris, Michel — Adobe, Eurographics 2026, [プロジェクト](https://eliemichel.github.io/dripping-thin-films/), [Wiley](https://onlinelibrary.wiley.com/doi/10.1111/cgf.70416)) — リアルタイムのグリッドベース薄膜流体シミュレーション。顔料を運ぶ流体層+アーティストが制御できる「垂れ」パラメータ。**Mixboxの真の後継に最も近い、このプロジェクトに最も関連の深い最新研究**
- **Lorentz Pigment Mixing (LPM)**(Ivković et al., 2025, [IntechOpen](https://www.intechopen.com/journals/1/articles/647), CC BY) — スペクトルデータなしでRGB空間内だけの自然な遷移を主張。Mixboxと直接ベンチマーク比較。現象論的(物理ベースではない)でマイナー誌だが、軽量な代替レシピ
- **MLによるK/Sカーブ生成**(Lars Wander, 2023, [記事](https://larswander.com/writing/spectral-paint-curves/)) — JAXの勾配降下で「望みの混色挙動を再現する架空顔料のK(λ)/S(λ)」を直接最適化。Colab+GLSL/JS実装付き。**実測データなしで自分専用の顔料セットを作れる**ため個人開発と相性が良い
- **Dichter 2023 "Kubelka-Munk model of full-gamut oil colour mixing"**([JAIC](https://www.aic-publishing.org/ojs/index.php/JAIC/article/view/277)) — Winsor & Newton油彩5色のK-Mキャリブレーション実例(33混色で平均ΔE00=1.49)。**仮想絵具を実チューブに合わせたい場合の実践ガイド**。同著者の解説 ["How Paints Mix"](http://www.kindofdoon.com/2019/07/how-paints-mix.html) も良入門
- **CoolerSpace**(OOPSLA 2024, [arXiv](https://arxiv.org/abs/2409.02771)) — 色空間演算の物理的正しさを型で保証する言語。色パイプラインの正しさの参考

## 6. 採用ガイド

| 選択肢 | 品質 | 速度 | 難易度 | ライセンス |
|---|---|---|---|---|
| Mixbox導入 | ◎(実測顔料ベース) | ◎(LUT) | ★☆☆ | 非商用のみ無料 |
| spectral.js移植 / WGM自作 | ○(単定数近似) | ○ | ★★☆ | **MIT/自由** |
| 二定数K-M自作(IMPaSTo流) | ◎(層・厚みまで) | △ | ★★★ | 自由(K/SデータはML生成で回避可) |

**推奨**: まず `mixbox_lerp` をブラシの色補間・スミア・グラデーションに差し替えて効果を体感(非商用なら即日)。商用化やOSS(GPL)化を視野に入れるなら spectral.js 方式を自前実装。層の重なり(グレーズ)まで表現したくなったら Curtis 1997 のRGB3チャンネルK-M層合成([03](03-layering-glazing-lifting.md))を追加するのが次の一手。

なお、**流体シミュレーション内での顔料表現を最初から「顔料濃度チャンネル」にしておけば**(RGBではなく)、混色は濃度の加算で自然に済み、表示時にのみ K-M/Mixbox でRGBへ変換する構成になる。これが最も物理に忠実で、Mixboxの潜在空間はまさにこの設計をRGBワークフローに後付けするためのもの。
