# 実装基盤の知見と参考実装

Rust + wgpu + egui で水彩シミュレーションのペイントツールを作るときに効く、基盤まわりの知見・参考実装・性能の目安。技術選定のときに比べた選択肢(TypeScript + WebGPU 案など)は [../memo/2026-07-03-implementation-stack.md](../memo/2026-07-03-implementation-stack.md) と [../memo/2026-07-03-rust-feasibility.md](../memo/2026-07-03-rust-feasibility.md)。

関連ノート: [01-fluid-simulation.md](01-fluid-simulation.md) / [02-pigment-mixing.md](02-pigment-mixing.md) / [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md)

---

## 1. GPU: wgpu

- **wgpu は WebGPU API 準拠の Rust ライブラリ**。ネイティブでは Vulkan / DirectX 12 / Metal / OpenGL ES バックエンドで動き、`wasm32` ターゲットではブラウザの WebGPU に直接マップされる([wgpu.rs](https://wgpu.rs/))。シェーダー言語は WGSL(内部の変換系は [naga](https://github.com/gfx-rs/wgpu/tree/trunk/naga))
- 水彩シミュの計算核心(2D テクスチャの compute 更新・ping-pong・storage texture)は wgpu の最も枯れた使い方で、**同じ WGSL がブラウザの WebGPU でもそのまま動く**
- ブラウザ由来の制約(タブごとの GPU メモリ制限、readback の非同期強制、タイマー精度制限)がネイティブにはない。保存・undo 用のテクスチャ readback は素直に書ける
- **wgpu / winit / egui はバージョンをロックステップで上げる**。メジャーバージョンが年に数回上がるので、egui の対応バージョンを起点に egui → egui-wgpu → wgpu → winit の順で合わせるのが楽
- wgpu の validation layer は厳格で、素の Vulkan/D3D12 より GPU プログラミングの誤りに早く気づける

## 2. ペン入力(筆圧)

- **筆圧だけなら追加クレートは要らない**。eframe 0.35(winit 0.30 系)の時点で winit が WM_POINTER を処理し、`GetPointerPenInfo` の筆圧を `Touch{force}` として届ける。egui-winit がそれを `egui::Event::Touch{force}` に変換する
- **[octotablet](https://github.com/Fuzzyzilla/octotablet) は Windows でハングする既知バグがある**。Manager を作ると RTS コールバックスレッド(COM マーシャリングで UI スレッドのメッセージポンプ待ち)と UI スレッド(`pump()` の mutex でコールバック待ち)が相互待ちし、アプリが「応答なし」になる(crates.io 0.1.0 / git master とも再現。[issue #18](https://github.com/Fuzzyzilla/octotablet/issues/18))。傾き・ホバー距離・消しゴム検出が欲しくなったときの候補だが、この修正の確認が前提
- winit 0.31 は [Pointer Event Overhaul(Issue #3833)](https://github.com/rust-windowing/winit/issues/3833) でマウス・タッチ・ペンを統一 Pointer イベントに集約し、ペン入力(Windows / Wayland / Web)を扱う
- **WinTab と Windows Ink**: 現代の Wacom/XP-Pen ドライバは Windows Ink をサポートしており、個人ツールなら Windows Ink 一本で足りる。WinTab は古いドライバ設定のユーザーを救う段階になってから([Krita Manual](https://docs.krita.org/en/user_manual/drawing_tablets.html)、[解説](https://docs.thesevenpens.com/drawtab/developers/wintab-vs-windows-ink))。低レベルのバインディングは [wintab_lite](https://github.com/thehappycheese/wintab_lite)
- ネイティブではイベントが OS のレートでそのまま届くので、Web の `getCoalescedEvents()` / `pointerrawupdate` に相当する手当ては要らない(Windows Ink は 133Hz 以上)

## 3. UI: egui / eframe

- キャンバスは egui の **`PaintCallback`** で任意の wgpu パスを UI 内に直接埋め込める。eframe + [egui-wgpu](https://docs.rs/egui-wgpu) で wgpu のデバイスとレンダラを共有する
- 即時モードなので、スライダーを山盛りにしたパラメータ調整パネルを作りやすい。シミュの品質はパラメータ調整の反復回数で決まるので、この相性が効く
- HTML/CSS に比べて凝ったビジュアルデザイン・テキスト処理・アクセシビリティは弱い
- 代替: [iced](https://github.com/iced-rs/iced)(Elm 風・保持モード)、[Slint](https://slint.dev/)(宣言的)

## 4. 混色クレート: mixbox

- **[mixbox クレート](https://crates.io/crates/mixbox)が公式にある**([scrtwpns/mixbox](https://github.com/scrtwpns/mixbox) に Rust 実装同梱)。`mixbox::lerp` / `rgb_to_latent` / `latent_to_rgb`。ライセンスは **CC BY-NC 4.0(非商用)**。ラッパーに [pigment-mixing-rs](https://github.com/virtualritz/pigment-mixing-rs)
- GPU 側は latent(7次元)を濃度比で線形混合し、latent→RGB を WGSL で評価する
- ライセンスフリーの路線(spectral.js / WGM、[02](02-pigment-mixing.md) §3)や KM 層合成([03](03-layering-glazing-lifting.md) §3)は数式の直接実装なので、CPU 参照実装を `cargo test` で検証して WGSL と突き合わせる進め方が取れる

## 5. Web(wasm)へ持っていくときの注意

- wgpu + egui/eframe は `wasm32-unknown-unknown` でビルドすればブラウザで動く。シェーダーとアルゴリズムはそのまま移る
- ペン入力はブラウザの Pointer Events 経由に戻り、ファイル保存も Web API 経由になる。**入力とストレージを trait で抽象化しておく**と移植が単純になる
- Web 側の筆入力は `PointerEvent.pressure` / `tiltX/Y`、間引かれた中間サンプルは [`getCoalescedEvents()`](https://developer.mozilla.org/en-US/docs/Web/API/PointerEvent/getCoalescedEvents)、低遅延は [`pointerrawupdate`](https://developer.mozilla.org/en-US/docs/Web/API/Element/pointerrawupdate_event) で取る

## 6. 参考になるオープンソース実装

### 流体・水彩シミュレーション

| リポジトリ | 内容 | 技術 |
|---|---|---|
| [PavelDoGreat/WebGL-Fluid-Simulation](https://github.com/PavelDoGreat/WebGL-Fluid-Simulation) | 定番のWebGL流体。MIT。splat 方式の入力の参考 | WebGL, ping-pong |
| [arsena21/writing-on-water](https://github.com/arsena21/writing-on-water) | WebGLでのデジタル水彩シミュレーションデモ(近接ストロークの混色に既知の問題あり) | WebGL + GLSL + JS |
| [inchkev/watercolor](https://github.com/inchkev/watercolor) | **Curtis et al. 1997 のC++実装**。単一顔料・2顔料ブレンド・リアルタイム操作のデモあり。論文PDF同梱 | C++ |
| [CalebKierum/Paint-Splatter](https://github.com/CalebKierum/Paint-Splatter) | 水彩の飛沫・混色・乾燥をシェーダーでリアルタイム再現。Van Laerhoven "Real-time simulation of watery paint" ベース。チュートリアル付き | Metal |
| [lisyarus/webgpu-shallow-water](https://github.com/lisyarus/webgpu-shallow-water) | 浅水方程式GPUソルバー(仮想パイプ法) | wgpu-native / C++ |
| [loicmagne/webgl2_fluidsim](https://github.com/loicmagne/webgl2_fluidsim) | Stable FluidsのWebGL2実装(約500行) | WebGL2 |
| [IRCSS/Compute-Shaders-Fluid-Dynamic-](https://github.com/IRCSS/Compute-Shaders-Fluid-Dynamic-) | Unity compute shaderでの2D流体 | Unity/HLSL |
| [Wumpf/blub](https://github.com/Wumpf/blub) | wgpuでの3D流体(PIC/FLIP/APIC) | Rust/wgpu |
| [jeantimex/webgpu-water](https://github.com/jeantimex/webgpu-water) | WebGPUのping-pong実例(read-after-write回避のため2ステップ/フレーム) | WebGPU |

### 混色(物理ベース)

- [scrtwpns/mixbox](https://github.com/scrtwpns/mixbox) — Kubelka-Munk理論に基づく顔料的混色。GLSL / HLSL / Metal / Unity / Godot / WebGL用シェーダー実装同梱(内部は3D LUTテクスチャ)。詳細は [02](02-pigment-mixing.md) §2
- 同アルゴリズムの原論文実装ミラー: [pigment-mixing](https://github.com/0xchaosbi/pigment-mixing)

### ブラシエンジン

- [mypaint/libmypaint](https://github.com/mypaint/libmypaint) — "brushlib"。C製、ISCライセンス。筆圧・速度等の入力を多数のブラシ設定にマッピングするデータ駆動エンジン。GIMP / Krita / OpenToonzで採用。使い方: [Using Brushlib wiki](https://github.com/mypaint/libmypaint/wiki/Using-Brushlib)
- Krita — [invent.kde.org/graphics/krita](https://invent.kde.org/graphics/krita)(GitHubミラー: [KDE/krita](https://github.com/kde/krita))。設計解説: [Krita/BrushEngine — KDE Community Wiki](https://community.kde.org/Krita/BrushEngine)

### Rust 製のグラフィックツール

- [Graphite](https://graphite.rs/)([GitHub](https://github.com/GraphiteEditor/Graphite))— Rust 製の 2D グラフィックエディタ

### 理論的基礎(論文)

- **Curtis et al. 1997 "Computer-Generated Watercolor"**(SIGGRAPH '97)— 水彩シミュレーションの原典。[プロジェクトページ](https://grail.cs.washington.edu/projects/watercolor/)、[論文PDF](https://www.cs.princeton.edu/courses/archive/fall00/cs597b/papers/curtis97.pdf)
- [GPU Programming for Real-time Watercolor Simulation(Texas A&M 修士論文)](https://oaktrust.library.tamu.edu/server/api/core/bitstreams/5575e4a6-40fd-4946-ad32-f83712ddc02f/content) — CurtisモデルのGPU化
- [Wetbrush: GPU-based 3D painting simulation at the bristle level(SIGGRAPH Asia 2015, NVIDIA)](https://dl.acm.org/doi/abs/10.1145/2816795.2818066) — 毛先レベルの筆シミュレーション

## 7. 性能の目安

| 事例 | シミュレーション規模 | 結果 |
|---|---|---|
| [PavelDoGreat/WebGL-Fluid-Simulation](https://github.com/PavelDoGreat/WebGL-Fluid-Simulation) | `SIM_RESOLUTION: 128`、`DYE_RESOLUTION: 1024`(モバイルはdye 512) | モバイル含め60fps。**速度場の解像度と表示色の解像度を分離するのが要点** |
| [Codrops: WebGPU Fluid Simulations (2025)](https://tympanus.net/codrops/2025/02/26/webgpu-fluid-simulations-high-performance-real-time-rendering/) | MLS-MPM 約10万パーティクル + Screen-Space Fluid Rendering | 統合GPUのノートPCや古いiPadでもリアルタイム動作 |
| [lisyarus/webgpu-shallow-water](https://github.com/lisyarus/webgpu-shallow-water) | 浅水方程式、256×256 | vsyncオフで余裕あり(ネイティブwgpu) |

グリッド型水彩シミュレーション(テクスチャ数枚×数パス/フレーム)なら、デスクトップGPUで512²は余裕、1024²も近年のdGPUなら60fps圏内。それより大きいキャンバスは、濡れているタイルだけを更新するアクティブタイルで計算量を濡れ面積に比例させる(本実装は 2048² までこれで成立させている)。
