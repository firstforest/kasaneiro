# 清書ペンの描き味:手法の調査

2026-11-03 の調査メモ。この時点の清書ペンは「入力点ごとの指数移動平均で手ぶれ補正(`pen_smooth`、`follow = 1 − 0.9×pen_smooth` で固定)→ centripetal Catmull-Rom で曲線補間 → 実効半径 = `pen_radius × mix(1, 筆圧^γ, 効き)` の円 splat を max 蓄積」([paint_core::brush::LineSmoother](../../crates/paint-core/src/brush.rs))。ペンを離すと、遅れている位置が終点まで追いつく(追いつき区間は最後の生筆圧で描く)。

ユーザーの判断: 下の「試す候補」のうち 1(速度で補正の強さを変える)と 2(追いつき区間で細らせて抜きを作る)を試す。

---

## 1. 手ぶれ補正

| 手法 | 仕組み | 特徴 |
|---|---|---|
| 1€ フィルタ(Casiez ほか, CHI 2012) | 速度で遮断周波数を変える1次の低域通過。`fc = mincutoff + beta×|速度|`、速度自体も低域通過してから使う | 遅い所は強く均してぶれを消し、速い所は弱めて遅れを減らす。パラメータ2つ。調整は beta=0 で mincutoff を決め、速く動かしたときの遅れが消えるまで beta を 10 倍刻みで動かす |
| Elasticurves(UIST 2011) | 「遅く描いた線は正確、速い線はノイズが多い」という観察から、速度と曲率で平滑化の強さを決める | 2025 年のスケッチ処理サーベイ(CGF)でもリアルタイム整形の基準手法として挙げられる |
| 紐引き(lazy brush、SAI / Krita の Stabilizer / Blender の Stabilize Stroke) | 筆先の周りに半径 R の遊び。ポインタが R の外に出たら、はみ出た分(距離 − R)だけ筆先を引く | R 内のぶれは完全に消える。平均型と違い角を残せる。遅れは常に約 R |
| Krita の重み付き平滑化 | 直近の点を距離で重み付け平均。離したあとの「尻尾」の太さを Tail Aggressiveness で決める(0 = 離した所で止まる / 0.15 前後 = ごく細い先 / 大きいほど太い先) | 追いつき区間の太さを専用の量で決めている |
| 2段補正(特許 US9990060 / US9898099、CSP のベクター後補正) | 描いている間は遅れの小さいフィルタ、離したあとに強いフィルタで引き直す | 「書いた線が後から変わるのは違和感がある」という指摘があり、直近の区間だけ引き直す特許もある(US9286703) |

## 2. 太さ・濃さの動き(入り抜き)

- **perfect-freehand**(tldraw / Excalidraw が使う。Rust 移植 `freedraw`): `thinning`(筆圧が太さに効く強さ。負で逆向き)、`streamline`(入力の平滑化)、`taper`(入り抜きの長さ。専用のイージング)。筆圧がないときは点の間隔(速度)から擬似筆圧を作る。
- **Jetpack Ink(androidx.ink)の BrushBehavior**: 入力(速度 cm/s・ブラシ径あたりの速度・傾き・入力からの経過時間・ストローク終了からの時間 など)をノードのグラフで先端の量(太さ倍率など)へ結ぶ。「筆圧 × 速度 × 時間」をデータで組み合わせる設計。
- **Ciallo**(SIGGRAPH 2024): 筆圧 → 太さ・不透明度は線形・二次の単純な関数。

## 3. 遅延

- インキング中に遅延を感じ始める閾値は約 50ms(Annett, Ng ほか GI 2014。インキング以外の操作は 2〜7ms)。感じ方はペン先とインクの距離ではなく、手やペンという視覚的な目印があるかで決まる。市販のペン環境は 65〜120ms(CHI 2014)。
- 予測入力: Android の Motion Prediction ライブラリ、等速・等加速度などのモデルを状態で切り替える特許群。予測した数ms先を仮に描き、実点が来たら置き換える。外れると予測なしより線がずれる。
- Windows の `DelegatedInkTrailVisual`: OS のコンポジタが仮の線を直接描く(Edge で遅延が最大 240% 改善)。winit + wgpu + egui の構成に入れるのは重い。
- 手ぶれ補正はどれも遅延を足すので、補正を強める前に今の遅延を測る価値がある。

## 4. 描画

- **Ciallo: GPU-Accelerated Rendering of Vector Brush Strokes**(SIGGRAPH 2024、HKUST(GZ) + Adobe Research): 太さ可変の線・スタンプ・エアブラシを、折れ線の線分ごとに GPU でラスタ化する。円 splat を並べず、線分ごとに解析的に覆い率を出す考え方は「1px 未満を薄く出す」「1本の中は均一な濃さ」と相性がよい。Windows 向けオープンソース試作あり。論文のストロークのパラメータ化に誤りがあると著者が認めている。

## 5. かさねいろへの当てはめ

- 生ポインタ点に時刻は無く(`PointerEvent` / `RasterStroke.points` は位置と筆圧だけ)、Undo/Redo はこの点列からの決定論的な引き直し。速度は「1入力点あたりの移動量」で代用するのが自然(Windows Ink はほぼ一定周期で点を出し、1フレームにまとめて届いた点も1点ずつ渡る)。点はテクセル座標なので、この速度は表示倍率に依存する。
- 今の `LineSmoother` は位置だけを遅らせ、筆圧は生の値を組にしている。そのため離す直前の筆圧の落ち込みが、遅れた位置(終点より手前)で起き、追いつき区間は最後の生筆圧のまま描かれる。

試す候補(効きそうな順):

1. `follow` を固定値から1€ フィルタの α(速度で変わる)に置き換える
2. 追いつき区間で太さを細らせて抜きを作る
3. 紐引き型を補正方式の選択肢にする
4. 遅延を測ってから予測入力を検討する
5. Ciallo 型の線分単位のラスタ化

## 出典

- [1€ Filter](https://cristal.univ-lille.fr/~casiez/1euro) / [解説](https://github.com/MKSharaf/OneEuroFilterExplained)
- [Elasticurves (UIST 2011)](https://www.dgp.toronto.edu/~ravin/papers/uist2011_elasticurves.pdf)
- [State-of-the-art Report in Sketch Processing (CGF 2025)](https://onlinelibrary.wiley.com/doi/10.1111/cgf.70079)
- [Filtering stylus strokes US9990060](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/9990060) / [Redrawing recent curve sections US9286703](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/9286703)
- [lazy-brush](https://github.com/dulnan/lazy-brush) / [Blender Stabilize Stroke #116468](https://projects.blender.org/blender/blender/issues/116468)
- [Krita freehand brush](https://docs.krita.org/en/reference_manual/tools/freehand_brush.html) / [Krita Tail Aggressiveness のコミット(2013)](https://mail.kde.org/pipermail/kimageshop/2013-June/011938.html)
- [perfect-freehand](https://github.com/steveruizok/perfect-freehand) / [freedraw](https://docs.rs/crate/freedraw/latest)
- [androidx.ink SourceNode.Source](https://developer.android.com/reference/kotlin/androidx/ink/brush/behavior/SourceNode.Source)
- [Android stylus / Motion Prediction](https://developer.android.com/develop/ui/views/touch-and-input/stylus) / [Ink lag compensation US9189147](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/9189147)
- [DelegatedInkTrailVisual](https://learn.microsoft.com/en-us/uwp/api/windows.ui.composition.delegatedinktrailvisual) / [WICG ink-enhancement](https://github.com/WICG/ink-enhancement)
- [Ciallo (Adobe Research)](https://research.adobe.com/publication/ciallo-gpu-accelerated-rendering-of-vector-brush-strokes)
- [How Low Should We Go? (GI 2014)](https://webdocs.cs.ualberta.ca/~wfb/publications/C-2014-GI-Latency.pdf) / [In the Blink of an Eye (CHI 2014)](https://webdocs.cs.ualberta.ca/~wfb/publications/C-2014-SIGCHI-Latency.pdf)
