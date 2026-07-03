# 重ね塗り(グレージング)と削り(リフティング)

「乾いた色の上に描いて綺麗な重なりを作る」機能と、「既に描いた色を削って白い部分を作る」機能のための技術ノート。調査日: 2026-07-03。

関連ノート: [01-fluid-simulation.md](01-fluid-simulation.md) / [02-pigment-mixing.md](02-pigment-mixing.md) / [04-implementation-stack.md](04-implementation-stack.md)

---

## 1. なぜ普通のアルファブレンディングでは水彩らしくならないか

- 通常のレイヤー合成(Porter-Duff の over 演算)は「上の色で下の色を隠す」加重平均であり、**光が上層を透過して下層で反射して戻ってくる**という水彩グレーズの光学挙動をモデル化していない。
- 水彩のグレージングは、乾いた層の上に薄い透明な層を重ねることで下の色が透けて**光学的に混色**される技法。Curtis et al. 1997 はこれを「内側から光るような (luminous / glowing from within)」効果と表現している。
- 安価な近似として **multiply(乗算)ブレンド**がある。透過フィルタとしての振る舞い(重ねるほど暗くなる、白は影響なし)は捉えられるが、顔料の散乱(S)を無視するため、不透明顔料の重なりや明るい色を上に置いたときの挙動が再現できない。品質順は概ね `over < multiply < Kubelka-Munk合成` で、コストもその順に上がる。

## 2. Curtis et al. 1997 のグレージングモデル

**"Computer-Generated Watercolor"** — Curtis, Anderson, Seims, Fleischer, Salesin, SIGGRAPH 1997。
[プロジェクトページ](https://grail.cs.washington.edu/projects/watercolor/) / [論文PDF](https://grail.cs.washington.edu/projects/watercolor/paper_small.pdf) / [ACM DL](https://dl.acm.org/doi/10.1145/258734.258896) / [特許 US6198489B1(同アルゴリズムの読みやすい全文)](https://patents.google.com/patent/US6198489B1/en)

- 絵全体を「紙の上の**順序付きウォッシュ(glaze)列**」として表現。各 glaze はピクセルごとの顔料量を持つデータ構造。
- **各 glaze は独立に流体シミュレーションを実行**して生成され、層間の流体相互作用はない(=前の層が完全に乾いた wet-on-dry を暗黙にモデル化)。
- 全 glaze の計算後、**Kubelka-Munk (KM) モデルで下から順に光学合成**して最終ピクセル色を得る。
- 「レイヤー確定=乾燥」というUIモデルにすれば、このグレージングモデルとそのまま一致する。**同一層内の混色は K,S の厚み加重平均、層間の重なりは R/T 合成式**、と役割が分かれているのが設計の要点。

## 3. Kubelka-Munk 層合成の式(論文 §5 より)

各顔料は RGB 3チャンネル分の吸収係数 K・散乱係数 S を持つ(計算はチャンネル独立)。

**単層の反射率 R・透過率 T**(厚み x の層):

```
R = sinh(b·S·x) / c
T = b / c
c = a·sinh(b·S·x) + b·cosh(b·S·x)
a = 1 + K/S,  b = √(a² − 1)
```

**2層の合成(Kubelka の合成式)** — 上層 (R₁,T₁)、下層 (R₂,T₂):

```
R = R₁ + T₁²·R₂ / (1 − R₁·R₂)
T = T₁·T₂ / (1 − R₁·R₂)
```

層間の無限回多重反射を等比級数で閉じた形。glaze を1枚追加するごとにこの式を畳み込み、最下層は紙の反射率とする。1つの glaze に複数顔料がある場合は、各顔料 k の K,S を相対厚み x_k で加重平均し、層厚は Σx_k。

**ユーザー指定色からの K,S 導出(§5.1)**: 実測の代わりに「単位厚みの顔料を白背景に塗った色 R_w と黒背景に塗った色 R_b」をユーザーが指定し、逆算する:

```
a = ½ · ( R_w + (R_b − R_w + 1) / R_b )
b = √(a² − 1)
S = (1/b) · coth⁻¹( (b² − (a − R_w)(a − 1)) / (b·(1 − R_w)) )
K = S·(a − 1)
```

- 各チャンネルで **0 < R_b < R_w < 1** を要求(ゼロ除算・NaN回避)。
- この指定法で顔料タイプを直感的に作り分けられる: **不透明**(例 Indian Red K=(0.46,1.07,1.50), S=(1.28,0.38,0.21))、**透明**(例 Quinacridone Rose K=(0.22,1.47,0.57), S=(0.05,0.003,0.03))、**干渉色**(高S・低K)。論文 Figure 5 に実値表あり。

**実装難易度は低い**。流体シミュレーションと完全に分離しており、光学合成は「ピクセルごと・チャンネルごとに sinh/cosh と有理式を数回」だけ。シェーダー1パスで書ける。

数値上の注意:
- `coth⁻¹(y) = ½·ln((y+1)/(y−1))`
- 厚み x が大きいと sinh/cosh がオーバーフローするので、指数形式に書き換えるか x をクランプ
- **計算はリニア色空間で行い、最後に sRGB へ**(論文には明記されていないが実装上重要)
- K,S が 1 を超えても実害なし(論文の実運用知見)

### 参考実装

| リソース | 内容 |
|---|---|
| [inchkev/watercolor](https://github.com/inchkev/watercolor) | Curtis 1997 の C++ 実装(MIT)。論文PDF同梱 |
| [rvanwijnen/spectral.js](https://github.com/rvanwijnen/spectral.js) | MIT。KM理論による絵具風混色(380–750nmスペクトル)。JS + GLSL。ただし混色のみで層合成機能はない |
| [raphlinus/kubelka](https://github.com/raphlinus/kubelka) | Raph Levien による KM ブレンディング探究 |
| [scrtwpns/mixbox](https://github.com/scrtwpns/mixbox) | KM を RGB-in/RGB-out に落としたライブラリ(→ [02-pigment-mixing.md](02-pigment-mixing.md)) |
| [TAMU修士論文: GPU Programming for Real-Time Watercolor Simulation](https://oaktrust.library.tamu.edu/server/api/core/bitstreams/5575e4a6-40fd-4946-ad32-f83712ddc02f/content) | Curtis手法のGPU化 |

## 4. 顔料の沈着・リフティングモデル(Curtis の吸着/脱着)

削り機能の土台になるのがこのモデル。ピクセルごと・顔料ごとに2つの量を持つ:

- `gs[p]` — 浅水層に**浮遊**している顔料
- `gd[p]` — 紙に**沈着**した顔料

毎ステップ、吸着(deposit, 浮遊→紙)と脱着(lift, 紙→浮遊)で交換する(特許 US6198489B1 の `AdsorbPigment` より):

```
Ddown (deposit) = gs[p] · deposit[p] · (1 − exposure[p]·b)        // 浮遊 → 紙
Dup   (lift)    = gd[p] · lift[p]    · (1 + exposure[p]·(b − 1))  // 紙 → 浮遊
// 各層が 1 を超えないようクランプした後:
gd[p] += Ddown − Dup
gs[p] += Dup − Ddown
```

`b` は局所的な紙の高さ ∈ [0,1]。顔料ごとの3つのスカラーが水彩の個性を決める:

| パラメータ | 論文での名前 | 意味 |
|---|---|---|
| `deposit[p]` | 密度 ρ | 沈着の速さ。重い顔料ほど早く沈着 |
| `lift[p]` | ステイニング力 ω の逆数 | 脱着の速さ。**高ステイニング顔料は lift が小さく、剥がれない** |
| `exposure[p]` | 粒状感 γ | 紙の高さ b が沈着/脱着をどれだけ変調するか |

### 粒状感 (granulation) の仕組み

deposit は `(1 − exposure·b)`、lift は `(1 + exposure·(b−1))` でスケールされるので、**紙の凸部(b→1)では沈着が抑制され剥離が促進、凹部(b→0)では沈着が最大**になる。exposure が大きい顔料ほど紙の谷に溜まる — 実際の粒状化顔料(ウルトラマリン、コバルト系など)の挙動と一致する。

物理的背景([handprint.com](https://www.handprint.com/HP/WCL/pigmt1.html)、[Winsor & Newton](https://www.winsornewton.com/blogs/guides/granulation-techniques-watercolour)): 粒状感とステイニングは**顔料の粒子サイズの表と裏**。粗く重い粒子 → 紙の谷に沈殿し(粒状化)、表面に載るだけなので剥がしやすい(非ステイニング)。サブミクロンの微細粒子(フタロ、キナクリドン等)→ 毛細管現象で紙繊維の奥に入り込み、乾くと剥がせない(ステイニング)が、均一に広がる(非粒状化)。

## 5. 削って白を出す機能 — なぜ単純な消しゴムでは不自然か

実際の水彩のリフティング(再湿潤して拭き取る)と比較すると、erase-to-transparent/white には4つの問題がある:

1. **ステイニングの床を無視する** — 実物では、ステイニング顔料は紙繊維に定着していて完全には剥がせず、必ず色が残る([handprint](https://www.handprint.com/HP/WCL/pigmt3.html)、[Daniel Smith](https://danielsmith.com/tutorials/staining-sedimentary-transparent-pigments/))
2. **エッジが機械的に硬い** — 実物は「再湿潤→拭き取り」の勾配なので縁が柔らかい([Winsor & Newton: How to lift watercolour](https://www.winsornewton.com/blogs/guides/how-to-lift-watercolour))
3. **剥がした顔料が消滅する** — 実物では剥がれた顔料は水に浮遊し、擦った領域の縁に再沈着して暗いリング(エッジダークニング)を作る
4. **紙テクスチャを無視する** — 実物は紙の凸部から先に剥がれ、谷には顔料が残る

### 自然な削りの実装レシピ

Curtis モデルに従えば、**削り = 沈着層から浮遊層への転送(削除ではない)**:

```
lifted = gd[p] · liftRate · (1 − ω[p]) · brushMask · texture(b)
gd[p] -= lifted
gs[p] += lifted   // その後、浮遊層が流動・再沈着・蒸発する
```

- **ステイン床を残す**: `gd[p]` を `ω[p] · gd_initial` 未満にクランプ。「削っても紙の白に完全には戻らない」ことが、消しゴムではなくリフティングに見える最大の手がかり
- **剥がした顔料をウェットマスクの境界に再沈着**させてエッジダークニングを再現
- ブラシマスクは**紙の高さフィールドで変調**し、ソフトな放射状フォールオフに
- デジタルならではの選択として、**「物理的なリフティング(色が残る)」と「完全消去(紙の白まで戻る)」を別ツールとして両方提供**するのが良い。Corel Painter がまさにこの構成(後述)

## 6. 商用ソフトの実装

### Corel Painter — 専用水彩レイヤー+明示的な乾燥コマンドの元祖

- [Watercolor layer](http://product.corel.com/help/Painter/540215550/Main/EN/Win-Documentation/Corel-Painter-Watercolor-Layer.html): 水彩ブラシで描くと**専用のWatercolorレイヤーが自動生成**され、水彩メディアが通常ピクセルから隔離される。コマンドは **New Watercolor Layer / Lift Canvas to Watercolor Layer / Wet Entire Watercolor Layer(拡散プロセスを再起動)/ Dry Watercolor Layer(乾いた面として扱う)**。
- [Water controls](http://product.corel.com/help/Painter/540215550/Main/EN/Win-Documentation/Corel-Painter-Water-controls.html): **Dry Rate**(拡散中の乾燥速度。低いほど広がる)と **Evap Thresh**(拡散が止まる最小水量)により、明示コマンドに加えて**シミュレーション時間での自然乾燥**も持つ。Evap Thresh はそのまま「シミュレーションを眠らせる条件」になる。
- Digital Watercolor(軽量版)の **Wet Fringe**(縁の暗まり)は**乾かすまでパラメトリックに再調整可能**で、乾燥時にピクセルへ焼き込まれる — 「湿った状態=パラメータ、乾いた状態=ラスタ」という分離の好例。
- [Real Watercolor controls](http://product.corel.com/help/Painter/540215550/Main/EN/Win-Documentation/Corel-Painter-Real-Watercolor-controls.html) に **Pickup** スライダー(「水が乾いた顔料をどの程度持ち上げられるか」)があり、Weight・Evaporation Rate・Settling Rate と並ぶ。さらに、水を加えず顔料を機械的に除去する **Wet Remove Density** / **Wet Eraser** を別ツールとして持つ。**水による再活性化と完全消去を別ツールにする**というUXの直接の参考例。

### Rebelle (Escape Motions) — 湿潤状態管理のリファレンス実装

- [Working with Water(Rebelle 8 マニュアル)](https://www.escapemotions.com/products/rebelle/manual/8.2/starting-painting/water/) の機能セット:
  - **Dry the Layer** — レイヤーを完全乾燥(絵・キャンバス・水すべて)。グレージング用の「焼き込み」コマンド
  - **Fast Dry** — **水だけを除去し、キャンバスの湿り気は残す**(絵具の湿りと紙の湿りを別の状態として持っている証拠)
  - **Wet the Layer / Wet All Visible** — レイヤー全体/描画部分だけを再湿潤
  - **Show Wet** — 湿り気を青のオーバーレイで可視化(デバッグ兼アーティスト向け)
  - **Pause Diffusion** — 流体シミュレーションの一時停止(乾燥より安価で可逆)
- 乾いた絵具は不活性ではなく、**水を塗ると再活性化して拡散に戻る**(リフティング対応)。ブレンドの規則は「下のキャンバスと色がどれだけ濡れているかで決まる。濡れているほど色が流れる」— wet-on-dry のグレーズは「新しいストロークが持ち込んだ水の分しか広がらない」ことで自然に成立する。
- [Watercolor tool properties](https://www.escapemotions.com/products/rebelle/manual/8.2/interface/panel-properties/watercolor/): **Water** スライダー、**Blend** モード(色を出さず既存色を混ぜる)、**Erase** モード(透明への消去)を分離。
- [Watercolor Granulation(Rebelle 5 Pro)](https://www.escapemotions.com/blog/rebelle-5-meet-color-pigments): 「乾燥中に徐々に現れる」粒状感。Strength/Density(各1–10)、3種の粒状テクスチャまたはユーザーPNG(1024×1024)、Texture Influence(0–10)。物理シミュレーションではなく**テクスチャで変調した沈着マスク**という、DiVerdi 流の手続き的アプローチに近い。ステイニングの明示的なパラメータはなく、absorbency/rewetting 経由で暗黙的に扱う。
- [開発者インタビュー(10 Years of Rebelle)](https://www.escapemotions.com/blog/10-years-of-rebelle-interview-with-developers): 初期の決定的な最適化は「**ストローク中は流体シミュレーションを一時停止し、CPUを描画に全振りする**」— ストローク堆積と拡散を別フェーズに分離する設計。

### Adobe Fresco — 「乾かない」という割り切り+ワンショット乾燥

- [Live brushes](https://helpx.adobe.com/fresco/using/live-brushes.html): 水彩 Live ブラシは「**決して乾かず**、ファイルを開き直しても混ざり続ける」。湿り気は時間ではなく永続的なレイヤー状態。
- ただしレイヤーオプションに **Dry Layer** コマンドがあり、それ以上のブレンドを止められる([チュートリアル](https://helpx.adobe.com/sk/fresco/how-to/watercolor-painting.html))。乾燥後の再湿潤はない。
- **UXの教訓**: Dry Layer がレイヤーメニューの奥にあり「にじみを止めたい瞬間に間に合わない」というユーザーの不満が公式フォーラムにある([スレッド](https://community.adobe.com/t5/fresco-discussions/watercolor-drying-tool/td-p/14730687))。前身の Adobe Sketch はワンタップの扇風機アイコンだった。**乾燥はタイミングが命の操作なので、常に見えるワンタップにすべき**。

### 乾いたレイヤー管理の共通アーキテクチャ(3製品に共通するパターン)

1. **レイヤーごとに複数バッファ**: 「乾いた沈着顔料」バッファ(合成・保存の対象)+「湿潤状態」(水量マップと湿った顔料バッファ。流体シミュレーションはこちらだけを操作する)
2. **シミュレーションは濡れたセルだけで走る**(Painter の Evap Thresh がまさにそのカットオフ)。さらにストローク堆積とは独立に一時停止できる(Rebelle の Pause Diffusion)
3. **「乾かす」=焼き込み**: 乾燥時に一度だけ「定着パス」(エッジダークニング、紙目への粒状感、バックラン)を走らせて湿った顔料を乾いたバッファへ合成し、湿潤バッファを解放する。乾いたレイヤーは以後コストゼロの通常ピクセルとして合成される — これがグレージングの成立条件
4. **新しいストロークは自分の水を持ち込む**: 乾燥後のブレンドは新ストロークが置いた湿り気だけで駆動される。乾いた顔料を水で再溶解させるか(Rebelle: yes、Fresco/Painter: no)は明示的な設計判断で、「no」でも古典的な wet-on-dry グレージングは成立し実装がはるかに単純
5. **レイヤータイプによる隔離**: Painter は専用レイヤー型、Fresco は永続的な per-layer 湿り気+ワンショット乾燥、Procreate はキャンバス状態を持たない(ストローク内 Wet Mix のみ)

**個人開発への適用**:
- 乾燥の主制御は**明示的な「乾かす」ボタン**(3製品とも搭載)。自動乾燥を入れるなら壁時計ではなく**シミュレーション時間の蒸発**として(Painter 方式 — シミュレーションの自然なスリープ条件も兼ねる)
- Rebelle の2段階(**Fast Dry**=水だけ除去 / **Dry**=完全乾燥)と **Wet the Layer / Show Wet** も安価に真似できて効果が大きい
- パフォーマンスの要: **ストローク堆積と拡散の分離**(拡散は低頻度ティックやストローク間に回す)+濡れた領域だけの更新

## 7. 手続き的な安価な近似(DiVerdi / Adobe 方式)

フル流体シミュレーションなしで見た目だけ再現する道もある。

**DiVerdi et al. "A Lightweight, Procedural, Vector Watercolor Painting Engine"** (I3D 2012 Best Paper, [ACM](https://dl.acm.org/doi/10.1145/2159616.2159627), [Adobe Research](https://research.adobe.com/publication/a-lightweight-procedural-vector-watercolor-painting-engine/))、拡張版 "Painting with Polygons" (IEEE TVCG 2013, [ACM](https://dl.acm.org/doi/10.1109/TVCG.2012.295))。ストロークを水ポリゴン+顔料ポリゴンのベクタとして保持し、手続き的な顔料移流でエッジダークニング・粒状感・再湿潤・バックラン等の「見た目」を再現。任意解像度でレンダリング可能。

Adobe の特許 [US10008011](https://patents.google.com/patent/US10008011) に具体的なレシピが明記されている:
- 粒状感 = **高周波 Perlin ノイズ × 低周波 Perlin ノイズ**の合成マスク
- エッジダークニング = 乾燥時に水の表面張力でストローク中心から縁へ顔料が移動する現象 → `darken = k · max(blur(alpha) − alpha, 0)` のような縁バンドへの顔料加算で近似

安価な近似のまとめ:
- 紙の高さテクスチャ h に対し `deposit ∝ (1 − γ·h)` の1行で Curtis の粒状感を近似(ピクセルごとの乗算のみ)
- 粒状マスクは湿り気/乾燥でゲートし「乾くときに現れる」ようにする(Rebelle 方式)
- ステイニングは色ごとのスカラー `stain ∈ [0,1]` とし、再湿潤時の再浮遊量を `(1 − stain)` に比例させる

## 8. 関連研究

- **Van Laerhoven & Van Reeth** "Real-time watercolor painting on a distributed paper model" (CGI 2004) / "Real-time simulation of watery paint" (CAVW 2005, [Wiley](https://onlinelibrary.wiley.com/doi/abs/10.1002/cav.95), [PDF](http://www.cs.ucf.edu/courses/cap6105/fall09/readings/watercolor_sim.pdf)) — Curtis の3層モデル(浅水+沈着+毛細管)をリアルタイム化。Worley ノイズベースの手続き的紙テクスチャで粒状感とエッジ効果を駆動。「水流が紙から顔料を持ち上げ、運び、再沈着させる」を再現。
- **Chu & Tai "MoXi"** (SIGGRAPH 2005, [ACM](https://dl.acm.org/doi/10.1145/1073204.1073221)) — 格子ボルツマン法による吸収性紙のインク拡散。エッジダークニングやフェザリングは出るが、乾いた層の再湿潤や多顔料の粒状感は対象外。詳細は [01-fluid-simulation.md](01-fluid-simulation.md)。

## 9. このプロジェクトへの示唆

1. **レイヤー構造は「レイヤー確定=乾燥」モデルが Curtis のグレージングと一致**し、要件「一度乾いた色の上に描くと綺麗に重なる」を最も素直に実現する。アクティブな湿レイヤーは1枚だけシミュレーションし、乾燥したら焼き込む。
2. 層合成は KM の R/T 合成式が本命。まず multiply で動かし、後から KM に置き換えるという段階的実装も可能。
3. 削り機能は、(a) Curtis 流のリフティング(ステイン床あり・エッジに再沈着・紙目で変調)と (b) デジタルならではの完全消去、の**2ツール構成**を推奨(Painter と同じ分離)。
4. 顔料ごとのパラメータは `density(沈着速度) / staining(剥がれにくさ) / granulation(紙目への反応)` の3つで水彩の個性のほぼ全てが表現できる。
