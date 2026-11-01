# 実装状況

マイルストーン M0〜M8・装備(H1〜H6、狙いは [design.md](design.md) §2)の現在地。**現在の実装状況の正典**で、書くのは現状だけ。実装の経緯・日付ごとの記録は [changelog.md](changelog.md) に残す。マイルストーン・装備を進めたとき、ツール・UI・ファイル形式・主要な仕組みが変わったとき、要目視確認の項目が増えた/片付いたときに、コード変更と同じコミットで更新する。

最終更新: 2026-11-01

## 1. 現在地

マイルストーン M0〜M8・装備 H1〜H6 はすべて完了している。以降は使い込みで出る要望と、[design.md](design.md) §4 の再検討トリガー待ち。

### 1.1 ツール構成

| 系統 | ツール | 要点 |
|---|---|---|
| 水彩(湿レイヤー) | 塗る(`tool`=0) | 筆の中身(水と4色の顔料)を置く。ツールバーの色スウォッチ = 筆に色を取る(押している間取り続けて濃くなる。2色取れば筆の中で混ざり、分離色にもなる)、水 = 浸して薄める(長く押すと透明)、洗う = 顔料 0・水満タン。どれも押すと塗るツールになる。なぞると中身が減り(`brush_hold`)、自動補充 on なら筆を下ろすたびに最後に作った中身へ戻る。置いたまま動かさなくても中身の水が尽きるまで色水が流れ出る(feed splat、満タンで `brush_charge` 秒) |
| | 削り(1) | リフト。沈着顔料を浮遊層へ戻す(ω が高い顔料は残る) |
| | 消す(2) | 水・速度・浮遊・沈着・濡れマスクをゼロへ(紙の白まで戻す) |
| | ぼかし筆(3) | 水を目標水位まで補い、ブラシ下の沈着顔料を溶かし戻す。馴染ませは毛細管拡散+顔料拡散の物理が担う |
| | 吸い取り(4) | 乾いた筆。表面の自由水と浮遊顔料を同率で取り除く(沈着は残る) |
| 線画 | 下書き鉛筆 / 清書ペン / ハイライト | 流体を通らないラスタ線画(各 r32float)。清書ペンの線は水の境界(透水率 `line_block`)。各ツールに消しゴムモード |
| 補助 | スポイト | 表示色をカーソル下から拾い、選択中スロットの基本色へ入れる。I キーを押している間は一時的にスポイト |
| | 一時消しゴム | E キーを押している間だけ、いまのレイヤーの消しゴム(水彩=消す / 線画=そのレイヤーの消しゴム) |
| | ブラシサイズ | カーソル位置に実寸の輪郭リング。Ctrl+Alt+左ドラッグで現ツールの半径を変更 |
| 乾燥操作 | 乾かす(固定)/ にじみを止める / 全体を濡らす | bake(乾燥レイヤーへ焼き込み)/ Fast Dry(浮遊を沈着へ落とし水と流れを止める)/ Wet the Layer(全面再湿潤) |
| 履歴 | 元に戻す / やり直し | 統一履歴(線画=多段・水彩=1段)。Ctrl+Z / Ctrl+Shift+Z(Ctrl+Y)。ストローク中は受け付けない。乾燥操作が成功すると水彩の1段 Undo は無効になる |

ツール番号の定義は [crates/paint-core/src/tool.rs](../crates/paint-core/src/tool.rs)(`WetTool` / `RasterTool`)、GPU 側の分岐は [splat.wgsl](../assets/shaders/splat.wgsl) と [linesplat.wgsl](../assets/shaders/linesplat.wgsl)。

### 1.2 UI 構成

- **上部メニュー**([src/app/ui/file_menu.rs](../src/app/ui/file_menu.rs)): 「ファイル」= 新規キャンバス…(512/1024/2048)/ 作品…(保存と開くの統合モーダル)/ 設定プリセット…/ 画像を書き出す (PNG) / 全部消す…、「ヘルプ」= かさねいろについて(バージョン・ライセンス)。破壊操作・同名作品への上書き保存・描きかけがある状態での作品読込は確認モーダルを挟む
- **左パネル**: 上部固定 = 乾燥3ボタン+元に戻す/やり直し。スクロール域 = 選択中レイヤーのツールだけ([src/app/ui/tools.rs](../src/app/ui/tools.rs) `active_tools_panel`。水彩 = ツールバー+スポイト+パレットパネル / 線画 = 各ツールのスライダー / 乾いた層 = 描画不可の案内・濃さの倍率スライダー(0〜2、1 に戻すボタンつき)・その層を描いたときの4色の取り込み)。開発モード時のみ末尾に味付け・診断・シミュ制御・ストローク記録再生・シェーダー状態。下端 = 操作結果のステータス、左下固定 = 開発モードトグル
- **パレットパネル**([src/app/ui/palette.rs](../src/app/ui/palette.rs)): 色ライブラリ(色相順のスウォッチグリッド。使用中マーク、右クリックでスロット指定読込・削除)+ パレット(4色一式。保存状態の表示、一覧から読込、「…」から削除、既定に戻す)。削除はサブメニュー内の確認ボタン経由
- **「色をつくる」モーダル**: ツールバーの色スウォッチをダブルクリックで開く(色・名前・ρ/ω/γ/μ・[この色をとっておく])。色スウォッチの右クリックで「ライブラリへ保存」。ダブルクリックの2回の押下で筆に取った色は取り消す
- **右パネル**: 表示(ズーム・回転)+ レイヤースタック([src/app/ui/layers.rs](../src/app/ui/layers.rs)。上=手前: ハイライト → 清書ペン → 下書き鉛筆 → 水彩 → 乾いた層 → 紙。選択 = ツール系統の切替、表示切替、乾いた層の並べ替えと下地への統合(⤓ = その層から下を下地に焼き固める。下地があると紙の行が「下地」になる)、合成方式)
- **キャンバス**([src/app/ui/canvas.rs](../src/app/ui/canvas.rs)): ホイール=カーソル中心に拡大、Shift+ホイール=15°回転、中ボタンまたはスペース+左ドラッグ=パン。空キャンバスに初回ガイド(最初の一筆・作品読込・再生開始で消える)。シェーダーエラーはオーバーレイ表示
- **筆の状態パネル**: 塗るツールのとき、ツールバーの下に常設(混ざった色を濃さに応じて薄めた丸い見本・濃さ・水のゲージ・顔料の色別積み上げゲージ(上限4回分)・色の内訳)。描いて減る/取って増える様子が毎フレーム追従する
- **永続化**(eframe storage): 開発モードの on/off、現行パレット、使っているパレットの保存名、筆の中身(自動補充の戻り先)と自動補充の on/off。初回起動の筆は洗った状態(水だけ)
- **フォント**: M PLUS 1p を埋め込み、欠けるグリフは Windows のシステム日本語フォントへフォールバック

### 1.3 ファイル形式と保存先

| 対象 | 場所 | 形式 |
|---|---|---|
| 作品 | `works/*.kasane`(git 管理外) | 独自バイナリ: `MAGIC "KSN1"` + メタ長 + メタ JSON(SimParams・現行パレット・レイヤー構成・レイヤーごとパレット・canvas_size・layer_count・has_base)+ 生ブロブ(f32 の湿3 → 乾燥 → 線画3 → 顔料 latent、下地があれば続けて f16 の下地2スライス)。`has_base` は省略時 false なので下地のない作品は同じ並びのまま。読込時に層数・構成・末尾の余分なバイトを検査し、サイズが違えばそのサイズでキャンバスを作り直す([src/work.rs](../src/work.rs)) |
| 設定プリセット | `assets/presets/*.json`(git 管理) | SimParams の JSON。`#[serde(default)]` で足したパラメータは既定値で埋まる |
| パレット(4色一式) | `assets/palettes/*.json`(git 管理) | `pigment::Palette`。同梱 4件(`default` は `default_palette()` と同値=テストで検査) |
| 色ライブラリ | `assets/pigments/*.json`(git 管理) | `pigment::Pigment` 1個 = 1ファイル、保存名 = 顔料名。同梱 24色 |
| ストローク記録 | `assets/strokes/*.json`(git 管理) | 生ポインタ入力+記録時パレット(`StoredRecording`)。同梱 [two-color-adjacent.json](../assets/strokes/two-color-adjacent.json) |
| PNG 書き出し | `snapshots/`(git 管理外) | キャンバス全体・無回転で焼く。ファイル名はタイムスタンプ+プリセット名 |
| リプレイ動画 | `videos/`(git 管理外) | 保存済みストローク記録を裏で再生した MP4(H.264・60fps・キャンバス全体・無回転)。ファイル名はタイムスタンプ+ストローク名。外部の ffmpeg を使う([src/video.rs](../src/video.rs)) |
| UI スクショ | `screenshots/ui-latest.png`(git 管理外) | 画面全体。`screenshots/request-shot` を書くと実行中アプリが撮影する |

- 保存名はすべて [src/assets.rs](../src/assets.rs) の `sanitize_name` を通し(パス区切り・Windows 禁止文字・制御文字を `_`、予約デバイス名は `_` 前置、空なら保存しない)、`{name}.{ext}.tmp` → rename のアトミック書き込みで保存する
- 後方互換を持つのは SimParams(プリセット)の `serde(default)` だけ。作品・ストローク記録・パレットは現行形式のみ読む
- 配布ビルド(feature `embed-assets`)は WGSL と既定 JSON を埋め込み、パス基準を exe のディレクトリにする([architecture.md](architecture.md) §11)

### 1.4 主要な仕組み

- **水**: Curtis 1997 簡略版(速度更新 → 発散緩和 → 移流)+ 水量そのものの毛細管拡散([note/05](note/05-wet-spreading.md))。吸着は水膜が切れるまで沈まないしきい値型、薄い水膜は流れを止める(`FLOW_MIN`)
- **筆の中身**: [crates/paint-core/src/load.rs](../crates/paint-core/src/load.rs) の `BrushLoad`(水 1 = 満タン、顔料 1 = 標準の1回分)。塗る splat はその時点の中身を運び(`Splat.water` / `Splat.pigment`、48B)、splat.wgsl が `brush_water` / `brush_pigment` を掛けて4スロットへ注ぐ。水と顔料は同じ割合で減るので、濃さは取る・浸す操作だけで変わる
- **顔料**: 4スロット(テクスチャ rgba = 4顔料)。顔料個性 ρ(沈みやすさ)・ω(染みつき)・γ(粒状感)・μ(粒の細かさ)。μ の差が分離色を生む
- **混色・発色**: 層内は mixbox の latent 混合、層間は Kubelka-Munk の R/T 合成(`compose_mode` 既定 1、0 で multiply)。mixbox 依存は `crates/pigment` に隔離
- **乾燥とレイヤー**: 湿レイヤー1枚 + 乾燥レイヤー最大8枚(顔料濃度のまま保持)。乾かす瞬間のパレットを GPU の latent 枠と CPU の `layer_palettes` に記録するので、後でパレットを変えても乾いた層の色は変わらない。8枚に達したら、下の層を**下地**(乗算・KM 両方式の合成結果を焼いた rgba16float 2スライス)へ統合して枠を空ける。見た目は変わらないが、統合した層の並べ替え・表示切替・色の取り込み・濃さの変更はできなくなり、紙色・`pigment_density`・層ごとの濃度倍率もそのときの値で固まる
- **乾いた層の濃さ**: 層ごとの濃度倍率(`DriedLayer::density`、既定 1)を合成時に顔料濃度へ掛ける。テクスチャは書き換えないのでいつでも戻せる。KM 合成では薄くするほど下が透ける。作品のレイヤー構成に保存する
- **紙**: 紙ハイト(値ノイズ4成分)はあるが、シミュ側の紙の質感(`paper_amp` / `paper_gran` / `paper_wet`)は既定 0。鉛筆の粒状感(`pencil_gran`)とリフトの紙凸部変調は紙ハイトを使う
- **計算範囲**: アクティブタイル(16×16 テクセル単位)で、まだ動きうる濡れたセル(水・速度・浮遊顔料のどれかが残る)があるタイルと流体 splat の近傍だけを計算する(`active_tiles`=0 で全面計算)。蒸発で水が尽きたタイルは、浮遊顔料が沈み切った数秒後に計算から外れる(濡れマスクは残るが、そのセルは全パスで不動点なので結果は全面計算と同じ)。計算しないタイルは ping-pong の2枚が揃っていれば書き込みも省く(タイルの3状態 COLD / COPY / ACTIVE と dirty フラグ)。キャンバスは 512 / 1024 / 2048 の正方形
- **表示**: 乾燥レイヤーの合成(multiply / KM)はキャッシュ(rgba16float 1枚)に焼いておき、毎フレームは湿レイヤーだけを発色して重ねる。焼き直すのは乾燥レイヤー・並び・可視性・濃度倍率・`pigment_density`・`compose_mode` が変わったときだけ
- **性能**(Apple Silicon / Metal、headless 計測 [tests/perf_bench.rs](../tests/perf_bench.rs)、1フレーム): 2048² で乾いた紙 2.4ms・一部濡れ(320²)2.7ms・全面計算 16.7ms・全面を濡らして蒸発し切った後 2.8ms、表示はキャンバス解像度で 0.45ms(乾燥レイヤーの枚数によらない)。残る乾いた紙のコストは、何もせずに戻るワークグループを全面に起動する分
- **時間**: シミュは 1/60 秒の固定刻みで進む(`paint_core::clock::SimClock` がフレームの経過時間を刻み数に変え、1フレーム最大4刻み)。1刻みのステップ数が速度倍率。ストローク記録の時刻も刻み単位なので、乾く速さ・再生のテンポはモニタのリフレッシュレートに依らない。headless は1フレーム=1刻み
- **乾き方**: `evap_rate` 既定 0.0003 = 水量 0.5 の塗りが約1分濡れたまま
- **パラメータ**: `SimParams` は 55 フィールド+末尾パディング1 = 224B。Rust と common.wgsl の struct の一致は [tests/sim_params_layout.rs](../tests/sim_params_layout.rs) が検査。Rust と WGSL で共有する定数(MAX_LAYERS・latent 枠・タイル)は [src/gpu/prelude.rs](../src/gpu/prelude.rs) がプレリュードとして生成する
- **テスト**: `cargo test --workspace` 全84件(kasaneiro crate 27 / km 6 / pigment 7 / paint-core 22 / shader_compile 1 / sim_params_layout 1 / e2e_watercolor 11 / e2e_ui 7 / e2e_video 2)+ `#[ignore]` の計測1件(perf_bench)。e2e は GPU 必須

### 1.5 既知の課題・制約

- エッジダークニング(`dry_edge`)は効きが弱い。濃い塗りでは被覆率が飽和して縁が目立たない(重要度が低いので許容。強めるなら縁バンドへ内側から顔料を移す質量保存つき再分配)
- FlowOutward(`edge_eta`)は既定 0 でパスごと省略している(顔料が縁でなく中心へ寄る定式化のまま)
- `paper_gran`=0 の既定では顔料個性 γ(粒状感)がほぼ効かない
- 筆の含み(feed splat)は生ポインタ点を記録するストローク記録(H5)では再現されない
- 色を持ち替えるには、洗ってから取る(色スウォッチを押すと筆の中の色に足される)。ワンクリックの持ち替えが欲しくなったら、修飾キー付きの押下を「洗ってから取る」に当てる
- 作品を開くと保存時の SimParams に戻る(保存時の `evap_rate` 等も含む)
- プリセットに `tool` / `brush_channel` / `display_mode` / `display_gain` / `compose_mode` などの作業状態も入る(プリセット読込で選択中ツール・顔料・診断表示まで切り替わる)。外すなら `#[serde(skip)]`。「作業状態ごと復元したい」なら現状が正なので、気になったときに決める
- 任意の技術的負債(単独でやる必要はない):
  - 乾燥レイヤーは `GpuCanvas.layers: Vec<DriedLayer>` を UI が直接編集して `sync_layers()` を手で呼ぶ規約結合。呼び忘れが将来バグになりうるので、レイヤー編集 UI に手を入れるときに編集メソッドと GPU 同期を内部に持つ `LayerStack` へ閉じる
  - GPU の読み戻しが [src/gpu/snapshot.rs](../src/gpu/snapshot.rs)(PNG)と [src/gpu/persist.rs](../src/gpu/persist.rs)(作品保存)に inline で重複している。読み戻し箇所が増える、またはキャンバスサイズの 64 の倍数制限(行バイト数の 256B 整列を保証している)を外すときに `gpu/readback.rs` へ寄せる

### 1.6 要目視確認

**筆の中身(2026-11-01)**:

- 洗う → 青を押す → 描くと青が標準の濃さで乗る。青を長めに押すと濃く、水を押すと薄くなり、筆の中身の見本・濃さの表示も追従する
- 水を1秒あまり押し続けると「筆: 水だけ」になり、その筆では色が乗らない
- 黄と青を続けて押すと見本が緑になり、描いた線も緑になる。フタロブルーとバーントシェンナを取って濡れた紙に置くと分離色に分かれる
- 長いストロークの終わりがやや淡くなる(含みの持ち 2000px)。自動補充 off で何本か引くと、だんだん薄く細くなる
- 置いたままの色水が、満タンの筆なら約2秒で止まる。描いて減った筆ほど早く止まる
- 色スウォッチのダブルクリックで色をつくるが開き、筆の中身は開く前のまま
- 再起動しても筆の中身と自動補充の設定が残る。記録したストロークを再生すると、記録時の筆の中身(濃さ・混色)で描かれる

**リプレイの動画書き出し(2026-11-01)**:

- Windows で ffmpeg を PATH に通す / exe の隣に置く / `KASANEIRO_FFMPEG` で指定する、のどれでも書き出せ、コンソールウィンドウが開かない。ffmpeg が無いときは案内がステータスに出る
- 2048² の書き出し中も画面の描画が引っかからない(キャンバスがもう1枚ぶん GPU メモリを使う)
- 書き出した動画の色・テンポが画面の再生と同じに見える

**コードレビュー指摘への対応(2026-10-31)で残っているもの**:

- 同名で保存 → 上書き確認が出て、キャンセルなら保存されない
- 描いた後に作品を開く → 破棄確認が出る。読込後に作品名欄が埋まる
- `a/b:c` のような名前が `a_b_c` で保存され、名前欄にも反映される
- 作品読込・記録の再生開始で初回ガイドが消える
- 線画レイヤーで描いて、左上の元に戻す/やり直しで戻る
- アクティブタイル表示(診断モード7)で、Fast Dry 後に計算範囲が消える / 線画ツールで描いても水彩側のタイルが立たない / 筆を離したフレームでブラシ近傍のタイルが消える

**乾いた層の濃さ(2026-11-01)で残っているもの**:

- 乾いた層を選んで「濃さ」を動かすと、その層だけがすぐ薄く/濃くなり、上に重ねたグレーズ越しの見え方も追従する。「1 に戻す」で元の見た目に戻る。保存して開き直すと倍率が残っている。倍率を変えた層を ⤓ で統合しても見た目が変わらない

**固定刻み・乾いたタイルの停止・下地への統合(2026-11-01)で残っているもの**:

- 144Hz など 60Hz 以外のモニタで、乾く速さ・にじむ速さが 60Hz と同じに見える。重い場面(2048² で全面が濡れている)でも描き味が極端に遅れない(1フレーム最大4刻みで打ち切るので、それ以上重いと実時間より遅れる)
- 記録したストロークを別のリフレッシュレートで再生しても、記録時と同じテンポで描かれる
- 診断モード7で、筆を置いて放っておくと、水が尽きて数秒後にそのタイルが暗く(何もしない)戻る。乾き切った跡の隣に新しく水を置くと、湿った跡を伝って広がる様子が以前と同じ
- レイヤーパネルの ⤓ で下の層を統合しても見た目が変わらない(乗算・グレーズの両方で)。紙の行が「下地」になり、統合後にまた「乾かす」で焼ける。統合した作品を保存して開き直すと同じ見た目に戻る。選択中の乾いた層が統合されたら水彩へ、残った層なら同じ層の選択が保たれる

**性能改善(2026-11-01)で残っているもの**:

- 2048² で描き続けても表示が乱れない。Fast Dry・乾かす・水彩の Undo・作品読込の直後に、消したはずの色や水が一瞬戻らない
- 診断モード7で、Fast Dry・乾かすの直後の1フレームだけ全面が青(コピーだけ)になり、その後は暗く(何もしない)戻る
- 乾燥レイヤーの表示切替・並べ替え・`compose_mode` / `pigment_density` の変更がすぐ画面に反映される。強く拡大したとき乾燥レイヤーの境目の見え方が以前と変わらない(キャッシュをテクセル単位で焼いてから補間するため)
- Windows(DX12 / Vulkan)で 2048² の体感が 60fps に収まる
- Windows(DX12 / Vulkan)で起動でき、「全部消す」・線画 Undo が正しく消える(`CLEAR_TEXTURE` 対応・非対応の両経路)
- ストローク中に I・スペース・Ctrl+Alt を押す、または乾いた層へ切り替える → 筆が止まって色水が出続けない。線画はその後 Undo で消せる
- 消すでなでた芯が完全に白へ戻り、後から色がにじみ出ない
- ズーム・回転中に PNG 書き出し → キャンバス全体が無回転で出る(スポイトは見えている色を拾う)
- ペンでモーダル(色をつくる等)やポップアップの上をタップしても、裏のキャンバスに描かれない
- 線画レイヤー選択中・リフト以外のツール選択中に記録を再生 → 記録どおりのツールで水彩に描かれる

**それ以前の変更で目視確認の記録がないもの**(詳細は [changelog.md](changelog.md) の該当エントリ):

- 乾き方: 新規起動で塗ると約1分は色が動き続ける。2色の境界のグラデーションが穏やかに進み、置いた色水の届く距離は保たれる
- 吸い取り: 濡れた塗りをなでると水たまりと浮いた色が引いて明るく抜け、沈着済みの色は残る
- ぼかし筆: 乾いた塗りの境界をなでると色が浮いて馴染む。なでても濃くならない。ω の高い顔料の芯が残る
- パレット・色の管理: 再起動してもパレットが残る / 色ライブラリが色相順・使用中マーク付き / 右クリックの読込・削除 / パレットの保存状態表示と上書き保存 / 色スウォッチ右クリックの保存
- I / E キーの一時切替(名前欄の入力中は切り替わらない)、ブラシのカーソルリングと Ctrl+Alt+ドラッグ
- キャンバス回転(Shift+ホイール・スライダー)中もカーソル位置に描ける・色を拾える、はみ出しが背景色

## 2. マイルストーン

| マイルストーン | 状態 | 現状の仕組み |
|---|---|---|
| M0 実験ハーネス | ✅ 完了 | eframe + egui-wgpu の `PaintCallback` でキャンバスを描き、H1(WGSL ホットリロード)と H2(SimParams → uniform)で駆動する |
| M1a 水の浅水層 | ✅ 完了 | 水テクスチャ(rgba32float: 水量・速度・濡れマスク)を splat → 速度更新([velocity.wgsl](../assets/shaders/velocity.wgsl))→ 発散緩和 × `relax_iters`([relax.wgsl](../assets/shaders/relax.wgsl))→ セミラグランジアン移流([advect.wgsl](../assets/shaders/advect.wgsl))で ping-pong 更新。水と顔料が動くのは濡れマスク内だけで、乾いた紙との境界は壁(Neumann)。`wet_expand` で前線を育てられる(既定 0) |
| M1b 顔料層 | ✅ 完了 | 浮遊・沈着の rgba32float ペア。浮遊顔料は水と同じく移流し、[diffuse.wgsl](../assets/shaders/diffuse.wgsl) で水依存の重み付き拡散(× `diffuse_iters`)、[transfer.wgsl](../assets/shaders/transfer.wgsl) で吸着/脱着+蒸発。同じ diffuse パスで水自身も毛細管拡散する |
| M1c Mixbox 混色 | ✅ 完了 | rgba 4チャンネル = 4顔料スロット。CPU([crates/pigment/src/lib.rs](../crates/pigment/src/lib.rs))が顔料・紙の latent を計算して uniform で渡し、[display.wgsl](../assets/shaders/display.wgsl) が濃度比で latent を線形混合 → latent→RGB 多項式で発色する |
| M1d 紙ハイト(+FlowOutward) | ✅ 完了(FlowOutward は先送り) | 紙ハイトは [crates/paint-core/src/paper.rs](../crates/paint-core/src/paper.rs) が起動時に生成する r32float(値ノイズ4成分)。速度勾配(`paper_amp`)・にじみ拡張(`paper_wet`)・吸着(`paper_gran`)の3箇所を変調するが、いずれも既定 0。FlowOutward([flowout.wgsl](../assets/shaders/flowout.wgsl))は `edge_eta`=0 で dispatch ごと省略 |
| M1.5 筆圧 | ✅ 完了 | [src/input.rs](../src/input.rs) の `PointerSource`(マウス / ペン = egui Touch)。筆圧は Windows Ink → winit → egui Touch の標準経路で取り、`実効値 = 基準値 × mix(1, 筆圧^γ, 効き)` を半径・水量・顔料量に掛ける。ペン接地中はマウス入力を無視する |
| M2 乾燥とレイヤー | ✅ 完了 | 乾燥レイヤーは rgba32float texture array(最大8枚、顔料濃度のまま)。下の層は下地(`GpuCanvas::merge_into_base`)へ統合して枠を空けられる。「乾かす」= [bake.wgsl](../assets/shaders/bake.wgsl) で `(浮遊+沈着) × dry_shift × 粒状感ゲート × (1 + dry_edge×縁バンド)` を新スライスへ焼き、湿レイヤーをゼロに。Fast Dry([fastdry.wgsl](../assets/shaders/fastdry.wgsl))・Wet the Layer([rewet.wgsl](../assets/shaders/rewet.wgsl))で焼かずに止める/濡らし直す。焼き込みは一方通行 |
| M3 削り・顔料個性・KM 合成 | ✅ 完了 | 削り = リフト(`lift_strength` × (1−ω) × 紙凸ゲート)と消す(完全消去)の2ツール。顔料個性は physics uniform(binding 9)で吸着・脱着・拡散を per-channel に変調。合成は KM: 各層を白地/黒地に置いた発色から `R=R_b`、`T²=(R_w−R_b)(1−R_b)` を導いて下から光学合成(CPU 参照実装は [crates/km/src/lib.rs](../crates/km/src/lib.rs)) |
| M4 仕上げ | ✅ 完了 | ぼかし筆(tool=3。水の補充+沈着顔料の溶かし戻し `water_lift`)と吸い取り(tool=4。`absorb_rate`) |
| M4.5 線画とハイライト | ✅ 完了 | 鉛筆・ペン・ハイライトの r32float 3枚に [linesplat.wgsl](../assets/shaders/linesplat.wgsl) が read_write storage で直接蓄積(流体パスとは別の compute パス)。清書ペンの濃度から透水率 `1 − line_block×濃度` を velocity / diffuse に掛ける。display が色の上に鉛筆 → ペン → ハイライトを合成。多段 Undo/Redo は生ポインタ点+描画時の SimParams からの決定論的再ラスタライズ([src/app/linehist.rs](../src/app/linehist.rs)) |
| M5 パレット | ✅ 完了 | `pigment::Palette`(4スロット)をランタイム編集し、`GpuCanvas::set_palette` が physics と現行 latent 枠を `write_buffer`。パレットと色単体のライブラリ([src/palette_store.rs](../src/palette_store.rs) / [src/pigment_store.rs](../src/pigment_store.rs))、スポイト、「色をつくる」モーダル、乾いた層の記録パレットからの取り込み |
| M6 スケールする土台 | ✅ 完了 | 水彩の1段 Undo(ストローク開始時に湿レイヤー3枚を `wet_backup` へ GPU コピー)と線画の多段 Undo を `undo_stack` で統合。パン・ズーム・回転は display 専用の `ViewUniform`(binding 11)、ポインタ→テクセル変換 `screen_to_texel` も同じ写像。アクティブタイルは [tilescan.wgsl](../assets/shaders/tilescan.wgsl) → [tiledilate.wgsl](../assets/shaders/tiledilate.wgsl) が有効フラグを作り、各シミュパスが非アクティブなタイルを素通しする |
| M7 作品保存 | ✅ 完了 | 湿レイヤー・乾燥レイヤー・線画・レイヤーごとパレット・現行パレット・SimParams を `works/*.kasane` 1ファイルへ(§1.3)。GPU ⇄ f32 は [src/gpu/persist.rs](../src/gpu/persist.rs) の `export_state` / `import_state`(読み戻しは1回の submit と1回の完了待ち)。ping-pong の parity は保存せず読込時に `current=0`。Undo 履歴は保存せず読込で破棄 |
| M8 キャンバス 2048² | ✅ 完了 | キャンバス1辺は `CANVAS_SIZES = [512, 1024, 2048]`(既定 512、64 の倍数で readback の行が 256B 整列)。実行時の値は `GpuCanvas.size`、変更は `recreate_canvas` でキャンバスを丸ごと作り直す。テクセル密度は据え置き(広い紙) |

## 3. 装備(試行錯誤ループ)

| # | 装備 | 状態 | 実装場所 |
|---|---|---|---|
| H1 | WGSL ホットリロード | ✅ 完了 | 監視は [src/gpu/hot_reload.rs](../src/gpu/hot_reload.rs)(作成/変更イベントで再ビルド)、再ビルドは [src/gpu/mod.rs](../src/gpu/mod.rs) の `rebuild_pipelines`(プレリュード + common.wgsl + 本体を連結。全ファイルを読み切ってからエラースコープを push)、行番号補正は [src/gpu/shader_error.rs](../src/gpu/shader_error.rs)、エラーオーバーレイは [src/app/ui/canvas.rs](../src/app/ui/canvas.rs) の `error_overlay` |
| H2 | パラメータパネル | ✅ 完了 | [crates/paint-core/src/sim.rs](../crates/paint-core/src/sim.rs) の `SimParams` → uniform buffer。スライダーは [src/app/ui/tools.rs](../src/app/ui/tools.rs)(ツール別)と [src/app/ui/tuning.rs](../src/app/ui/tuning.rs)(開発モードの味付け) |
| H3 | プリセット保存/読込 | ✅ 完了 | [src/preset.rs](../src/preset.rs)(SimParams ⇄ JSON)+ [src/app/ui/file_menu.rs](../src/app/ui/file_menu.rs) の設定プリセットモーダル。同梱 [default.json](../assets/presets/default.json) ほかが全部読めることをテストで検査 |
| H4 | デバッグ表示切替 | ✅ 完了 | [display.wgsl](../assets/shaders/display.wgsl) の `display_mode` 分岐で8モード(通常 / 水量 / 速度場 / 湿りオーバーレイ / 浮遊顔料 / 沈着顔料 / 紙ハイト / アクティブタイル)。選択は開発モードの「表示・診断」([src/app/ui/tuning.rs](../src/app/ui/tuning.rs)) |
| H5 | ストローク記録・再生 | ✅ 完了 | モデルは [crates/paint-core/src/replay.rs](../crates/paint-core/src/replay.rs)、永続化は [src/replay.rs](../src/replay.rs)、UI は [src/app/ui/panels.rs](../src/app/ui/panels.rs) の `replay_panel`。補間前の生ポインタ入力(刻み番号・テクセル座標・筆圧。JSON のキー名は `frame`)をストローク単位のツール・顔料スロットと記録時パレットごと保存し、再生はキャンバスをリセットして記録時のテンポで流す。記録・再生は 1/60 秒の刻みごとに進み、再生中は毎刻み記録時のツール・スロットで splat を出し、水彩以外のレイヤーを選んでいれば水彩へ切り替える。保存済み記録の行の「🎬 動画」で MP4 に書き出せる([src/video.rs](../src/video.rs)。別スレッドの `HeadlessCanvas` で再生して1刻みずつ ffmpeg の stdin へ流す(1刻み = 動画の1フレーム、60fps)。キャンバスサイズ・パラメータは書き出し開始時の値、パレットは記録時のもの。再生後 2 秒の余韻まで撮る。進捗バーと取り消しあり。画面のキャンバスは変えない) |
| H6 | シミュレーション制御 | ✅ 完了 | 一時停止 / 1ステップ / 速度倍率(1刻みあたりのステップ数)/ UI スクショは開発モードの「シミュレーション制御」([src/app/ui/tuning.rs](../src/app/ui/tuning.rs))。PNG 書き出し・全部消すはファイルメニュー。PNG は display と同じシェーダーでオフスクリーンに焼いて読み戻す([src/gpu/snapshot.rs](../src/gpu/snapshot.rs))。UI スクショは egui の `ViewportCommand::Screenshot` で撮り、外部トリガーは `ScreenshotWatcher`([src/gpu/hot_reload.rs](../src/gpu/hot_reload.rs))が `screenshots/request-shot` を監視する |

## 4. 計画外でやったこと

- 日本語フォント: M PLUS 1p(OFL、[assets/fonts/](../assets/fonts/))を埋め込み、Windows のシステム日本語フォントをフォールバックに足す([src/app/mod.rs](../src/app/mod.rs) `install_japanese_font`)
- アセットの基準ディレクトリと保存の共通化: [src/assets.rs](../src/assets.rs) の `base_dir`(通常 = CARGO_MANIFEST_DIR / 配布 = exe 隣)、保存名の正規化・アトミック書き込み・JSON ストア共通関数
- WGSL 共通定義の連結ロード: SimParams / Splat の struct を [assets/shaders/common.wgsl](../assets/shaders/common.wgsl) に1箇所化し、各シェーダーの先頭に連結してコンパイルする。さらに前に Rust と共有する定数のプレリュード([src/gpu/prelude.rs](../src/gpu/prelude.rs))を足す
- WGSL コンパイル可能性テスト: naga でパース+検証する [tests/shader_compile.rs](../tests/shader_compile.rs)(assets/shaders を走査し、全キャンバスサイズのプレリュードで検証)
- SimParams のレイアウト検査: [tests/sim_params_layout.rs](../tests/sim_params_layout.rs)(Rust 定義と common.wgsl の struct の名前・順序・型の一致)
- 性能の計測: [tests/perf_bench.rs](../tests/perf_bench.rs)(`#[ignore]`。`cargo test --test perf_bench -- --ignored --nocapture` で 512/1024/2048 のフレーム時間と表示時間を表で出す)
- e2e テスト(体験の回帰検知): [tests/e2e_watercolor.rs](../tests/e2e_watercolor.rs) = ウィンドウなしの `HeadlessCanvas`([src/headless.rs](../src/headless.rs)。H5 の再生と同じ `Player` → `GpuCanvas::encode_frame` 経路)でストロークを再生し、wet-on-wet・グレージング・消去・削り・ステイニング差・同梱ストロークを色相/明度の帯で判定 / [tests/e2e_ui.rs](../tests/e2e_ui.rs) = egui_kittest で PaintApp 全体を起動し、マウス・ペン(筆圧)・ボタン・スウォッチのダブルクリック・乾かすを実入力で判定。要素はキャンバスと色スウォッチに付けた AccessKit 名(「キャンバス」「塗る色 N: 顔料名」)とボタン文字で引く。結果画像は `target/tmp/e2e/*.png`
- 通常/開発モードの二層化: 通常は描くための UI だけを出し、味付け・診断・記録再生・シェーダー状態は開発モードのトグルの裏に置く
- 配布ビルド(feature `embed-assets`): WGSL と既定 JSON を埋め込んだ exe 単体配布。「かさねいろについて」でライセンス一覧([assets/licenses/third-party.txt](../assets/licenses/third-party.txt))を表示する
