# 実装技術スタック

水彩シミュレーションペイントツールを個人開発するための技術基盤の調査まとめ。
想定: Windows上の個人用ツール。リアルタイムの筆描画+水彩のにじみ(流体シミュレーション)+物理的な混色。調査日: 2026-07-03。

関連ノート: [01-fluid-simulation.md](01-fluid-simulation.md) / [02-pigment-mixing.md](02-pigment-mixing.md) / [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md)

---

## 1. GPU計算基盤の選択肢と比較

### 1-1. WebGPU (compute shader) + ブラウザ / Electron / Tauri

**サポート状況(2025〜2026年時点)**

- 2025年11月時点で Chrome / Edge / Firefox / Safari の全主要ブラウザがWebGPUをデフォルト出荷済み。Chrome/Edge は 113 以降(WindowsではD3D12バックエンド)、Firefox は 141(Windows)、Safari は 26 から。
  - 出典: [web.dev — WebGPU is now supported in major browsers](https://web.dev/blog/webgpu-supported-major-browsers)、[MDN WebGPU API](https://developer.mozilla.org/en-US/docs/Web/API/WebGPU_API)
- **Windowsに限れば最も安定したプラットフォーム**(Chrome 113から2年以上の実績)。compute shader・storage buffer・`atomicAdd`等がフルに使える。
- **Electron**: Chromiumを同梱するためバージョンを自分で固定でき、ブラウザより先行して安定利用可能。Electron 32+ で実用段階との報告。
  - 出典: [electron/electron Issue #26944](https://github.com/electron/electron/issues/26944)
- **Tauri**: OSのWebView(WindowsはWebView2)を使うため、WindowsではWebGPUが動作する。ただしmacOS(WKWebView)/Linux(WebKitGTK)ではサポートが一貫せず、クロスプラットフォーム性は弱い。**Windows専用個人ツールなら実用圏内**。
  - 出典: [tauri-apps/tauri Issue #6381](https://github.com/tauri-apps/tauri/issues/6381)、[Issue #12846](https://github.com/tauri-apps/tauri/issues/12846)

### 1-2. WebGL2 fragment shader (ping-pongテクスチャ)

- compute shaderなしでも、**倍バッファのフレームバッファ間でテクスチャをping-pong**させ、移流→発散→圧力解法(Jacobi反復)→勾配減算を各fragment shaderパスで回す古典手法(GPU Gems Ch.38 / Jos Stam "Stable Fluids")が確立している。
- 実績: [PavelDoGreat/WebGL-Fluid-Simulation](https://github.com/PavelDoGreat/WebGL-Fluid-Simulation)(モバイルでも動作)、[loicmagne/webgl2_fluidsim](https://github.com/loicmagne/webgl2_fluidsim)(約500行)、[piellardj/navier-stokes-webgl](https://github.com/piellardj/navier-stokes-webgl)。
- 利点: どこでも動く・参考実装が最多。欠点: 共有メモリやatomicが使えず、Jacobi反復のパス数が多いとdraw call数が嵩む。粒子系(SPH/MPM)には不向き。

### 1-3. ネイティブ: wgpu (Rust) / Unity / Godot

- **wgpu (Rust)**: WebGPU API準拠のクロスプラットフォームGPUライブラリ([wgpu.rs](https://wgpu.rs/)、[gfx-rs/wgpu](https://github.com/gfx-rs/wgpu))。流体実績として [Wumpf/blub](https://github.com/Wumpf/blub)(PIC/FLIP/APIC 3D流体)、[lisyarus/webgpu-shallow-water](https://github.com/lisyarus/webgpu-shallow-water)(浅水方程式を仮想パイプ法で解く、デフォルト256×256グリッド)。ブラウザ制約なしで最高性能だが、UI(カラーピッカー、レイヤーパネル等)を自作する負担が大きい(egui/iced等が必要)。
- **Unity compute shader**: [IRCSS/Compute-Shaders-Fluid-Dynamic-](https://github.com/IRCSS/Compute-Shaders-Fluid-Dynamic-)(2D流体実装、解説記事: [Gentle Introduction to Realtime Fluid Simulation](https://shahriyarshahrabi.medium.com/gentle-introduction-to-fluid-simulation-for-programmers-and-technical-artists-7c0045c40bac))。エディタとUIが最初からあるのが利点。
- **Godot**: 4.xでcompute shader対応。ただし水彩ペイント用途の実績はスタイライズドシェーダー程度で、シミュレーション実績は薄い。

### 比較まとめ

| 基盤 | Windows安定性 | compute | UI開発 | 参考実装 | 備考 |
|---|---|---|---|---|---|
| WebGPU + ブラウザ/Electron | ◎ | ○ | ◎ (HTML/CSS) | 増加中 | 2025年に全ブラウザ対応済 |
| WebGL2 ping-pong | ◎ | ×(fragmentで代替) | ◎ | 最多 | 枯れていて確実 |
| wgpu (Rust) | ◎ | ◎ | △ (自作) | 中 | 最高性能・学習コスト大 |
| Unity | ◎ | ◎ | ○ | 中 | ランタイムが重め |

---

## 2. 参考になるオープンソース実装

### 流体・水彩シミュレーション

| リポジトリ | 内容 | 技術 |
|---|---|---|
| [PavelDoGreat/WebGL-Fluid-Simulation](https://github.com/PavelDoGreat/WebGL-Fluid-Simulation) | 定番のWebGL流体。約16.2k★、MIT | WebGL, ping-pong |
| [arsena21/writing-on-water](https://github.com/arsena21/writing-on-water) | WebGLでのデジタル水彩シミュレーションデモ(近接ストロークの混色に既知の問題あり) | WebGL + GLSL + JS |
| [inchkev/watercolor](https://github.com/inchkev/watercolor) | **Curtis et al. 1997 のC++実装**。単一顔料・2顔料ブレンド・リアルタイム操作のデモあり。論文PDF同梱 | C++ |
| [CalebKierum/Paint-Splatter](https://github.com/CalebKierum/Paint-Splatter) | 水彩の飛沫・混色・乾燥をシェーダーでリアルタイム再現。Van Laerhoven "Real-time simulation of watery paint" ベース。チュートリアル付き | Metal |
| [lisyarus/webgpu-shallow-water](https://github.com/lisyarus/webgpu-shallow-water) | 浅水方程式GPUソルバー(仮想パイプ法) | wgpu-native / C++ |
| [loicmagne/webgl2_fluidsim](https://github.com/loicmagne/webgl2_fluidsim) | Stable FluidsのWebGL2実装(約500行) | WebGL2 |
| [IRCSS/Compute-Shaders-Fluid-Dynamic-](https://github.com/IRCSS/Compute-Shaders-Fluid-Dynamic-) | Unity compute shaderでの2D流体 | Unity/HLSL |
| [Wumpf/blub](https://github.com/Wumpf/blub) | wgpuでの3D流体(PIC/FLIP/APIC) | Rust/wgpu |
| [jeantimex/webgpu-water](https://github.com/jeantimex/webgpu-water) | WebGPUのping-pong実例(read-after-write回避のため2ステップ/フレーム) | WebGPU |

### 混色(物理ベース)

- **[scrtwpns/mixbox](https://github.com/scrtwpns/mixbox)** — Kubelka-Munk理論に基づく顔料的混色ライブラリ(RGB in → RGB out)。約3.5k★。**GLSL / HLSL / Metal / Unity / Godot / WebGL用シェーダー実装同梱**(内部は3D LUTテクスチャ)。C/C++/C#/Java/JS/Python/Rust対応。Rebelle 5 Proにも採用。**ライセンスはCC BY-NC 4.0(非商用のみ)** — 個人開発・非商用なら利用可、将来販売するなら商用ライセンス要([scrtwpns.com/mixbox](https://scrtwpns.com/mixbox/))。
- 同アルゴリズムの原論文実装ミラー: [pigment-mixing](https://github.com/0xchaosbi/pigment-mixing)。

### ブラシエンジン

- **[mypaint/libmypaint](https://github.com/mypaint/libmypaint)** — "brushlib"。C製、ISCライセンス。筆圧・速度等の入力を多数のブラシ設定にマッピングするデータ駆動エンジン。GIMP / Krita / OpenToonzで採用。使い方: [Using Brushlib wiki](https://github.com/mypaint/libmypaint/wiki/Using-Brushlib)。
- **Krita** — 本体は [invent.kde.org/graphics/krita](https://invent.kde.org/graphics/krita)(GitHubミラー: [KDE/krita](https://github.com/kde/krita))。設計解説: [Krita/BrushEngine — KDE Community Wiki](https://community.kde.org/Krita/BrushEngine)。`KisPaintOp`基底クラスの `paintAt / paintLine / paintBezierCurve`、共通部品の `libpaintop` という構成はブラシエンジン設計の教科書として有用。タブレット入力はWinTab/Windows Ink両対応。

### 理論的基礎(論文)

- **Curtis et al. 1997 "Computer-Generated Watercolor"**(SIGGRAPH '97)— 水彩シミュレーションの原典。[プロジェクトページ](https://grail.cs.washington.edu/projects/watercolor/)、[論文PDF](https://www.cs.princeton.edu/courses/archive/fall00/cs597b/papers/curtis97.pdf)
- [GPU Programming for Real-time Watercolor Simulation(Texas A&M 修士論文)](https://oaktrust.library.tamu.edu/server/api/core/bitstreams/5575e4a6-40fd-4946-ad32-f83712ddc02f/content) — CurtisモデルのGPU化
- [Wetbrush: GPU-based 3D painting simulation at the bristle level(SIGGRAPH Asia 2015, NVIDIA)](https://dl.acm.org/doi/abs/10.1145/2816795.2818066) — 毛先レベルの筆シミュレーション

---

## 3. キャンバス描画アーキテクチャ

### 3-1. シミュレーショングリッド構成(Curtis 1997モデルのGPU化が定石)

Curtisモデルは3層構造で、これをテクスチャ群にマップする:

1. **浅水層 (shallow-water layer)**: 水の速度場 `(u, v)` と水深/湿り気 — RG(BA)テクスチャ1枚
2. **顔料層**: 水中を移動する顔料濃度 — 顔料ごとに1チャンネル(RGBA1枚で4顔料)。移流・拡散させる
3. **沈着層 (pigment-deposition layer)**: 紙に定着した顔料 — 同じくRGBAテクスチャ。吸着/剥離(absorption/lifting)で顔料層と交換
4. **紙テクスチャ**: 紙の高さ・繊維方向 — 静的テクスチャ1枚。エッジダークニングや粒状感(granulation)の源

各ステップ(移流→圧力解法→顔料移動→吸着→蒸発)を ping-pong(WebGL2)または storage texture への直接書き込み(WebGPU compute)で回す。

### 3-2. レイヤー合成

- Curtis方式: レイヤー(グレーズ)ごとに独立シミュレーションし、**Kubelka-Munkの光学合成**で重ね合わせる。
- 実用簡略版: シミュレーション対象は「アクティブな湿レイヤー」1枚のみとし、乾燥したら通常のレイヤー(RGBA)に焼き込み、合成時の色混合に **Mixbox の `mixbox_lerp`(シェーダー版)** を使うと、乾燥後合成でも顔料的な発色(青+黄=緑)が得られる。個人開発ではこの構成が現実的。
- 詳細は [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md) を参照。

### 3-3. 筆ストローク入力(ペンタブ・筆圧)

Web/Electron系なら **Pointer Events Level 3** で完結する:

- `PointerEvent.pressure`(0–1正規化筆圧)、`tiltX/tiltY`、`tangentialPressure`、`twist` — [MDN PointerEvent](https://developer.mozilla.org/en-US/docs/Web/API/PointerEvent)
- **`getCoalescedEvents()`**: ブラウザがフレーム間で間引いた中間サンプルを全て取得。滑らかなストローク補間に必須 — [MDN getCoalescedEvents](https://developer.mozilla.org/en-US/docs/Web/API/PointerEvent/getCoalescedEvents)
- **`pointerrawupdate`**: rAFを待たずに高頻度で入力を受ける低遅延イベント — [MDN pointerrawupdate](https://developer.mozilla.org/en-US/docs/Web/API/Element/pointerrawupdate_event)、[W3C Pointer Events 3](https://www.w3.org/TR/pointerevents3/)
- WindowsのWacom等はWindows Ink経由でpressure/tiltがPointer Eventsに乗る。ネイティブ(Rust/Unity)の場合はWinTab/Windows Ink対応を自前実装する必要があり(Kritaはこの二重対応をしている)、これはWeb系を選ぶ大きな理由になる。
- ストローク→シミュレーションへの入力は「ブラシスタンプ位置に水+顔料+速度インパルスをsplatする」方式(PavelDoGreatのsplat実装が参考)。

---

## 4. パフォーマンスの目安

実測・公開値ベース:

| 事例 | シミュレーション規模 | 結果 |
|---|---|---|
| [PavelDoGreat/WebGL-Fluid-Simulation](https://github.com/PavelDoGreat/WebGL-Fluid-Simulation) | デフォルト `SIM_RESOLUTION: 128`、`DYE_RESOLUTION: 1024`(モバイルはdye 512に低減) | この設定でモバイル含め60fps動作。**速度場の解像度と表示色の解像度を分離するのが要点** |
| [Codrops: WebGPU Fluid Simulations (2025)](https://tympanus.net/codrops/2025/02/26/webgpu-fluid-simulations-high-performance-real-time-rendering/) | MLS-MPM 約10万パーティクル + Screen-Space Fluid Rendering | 統合GPUのノートPC(Ryzen 7 5825U)や6年前のiPad Air 3でもリアルタイム動作 |
| [lisyarus/webgpu-shallow-water](https://github.com/lisyarus/webgpu-shallow-water) | 浅水方程式、デフォルト256×256 | vsyncオフで余裕あり(ネイティブwgpu) |
| [Vitalify: WebGL & WebGPU流体比較](https://www.vitalify.asia/blog/xr-3d-web/fluid-simulation-webgl-webgpu-real-time-en) | — | WebGLは1万ノード超で60fps維持が苦しくなるが、WebGPU computeなら大幅に余裕、との比較報告 |

**目安のまとめ**: グリッド型水彩シミュレーション(テクスチャ数枚×数パス/フレーム)なら、**デスクトップGPUで512²は余裕、1024²も近年のdGPUなら60fps圏内**。WebGL2 fragment方式は圧力Jacobi反復(20〜50回)のパス数がボトルネックになるため、シミュレーション解像度は256〜512²に抑えて表示解像度と分離するのが定石。WebGPU computeなら共有メモリとatomicで反復あたりのコストを下げられ、より高解像度が狙える。

---

## 5. 個人開発での推奨構成

**第一候補(推奨): TypeScript + WebGPU compute shader、配布はElectron(またはChrome/Edgeでそのまま)**

- 理由:
  1. Windows上のChromium系WebGPUは2年以上の実績で安定、Electronならバージョン固定可能
  2. ペンタブ入力がPointer Events(`pressure`/`getCoalescedEvents`/`pointerrawupdate`)だけで済み、WinTab対応を自作しなくてよい
  3. UI(カラーピッカー・レイヤーパネル)をHTML/CSSで作れる
  4. MixboxにWebGL/GLSLシェーダー実装があり混色をそのまま組み込める
- 構成案:
  - シミュレーション: WGSL compute、浅水+顔料移流(Curtis 1997簡略版)。グリッド512²から開始、表示は別解像度
  - 混色: Mixbox(非商用ならCC BY-NC 4.0で無料)。商用化の可能性があるなら自前Kubelka-Munk実装(pigment-mixing リポジトリ参照)に差し替え
  - ブラシ: splat方式から始め、パラメータ設計はlibmypaint/Kritaの`KisPaintOp`構造を参考に
- フォールバック: WebGPU非対応環境向けにWebGL2 ping-pong版(PavelDoGreatの構造を流用)を用意するか、割り切って要求環境をChromium系に固定

**第二候補: Rust + wgpu ネイティブ** — 最大性能と将来のポータビリティ(同じWGSLをWebにも持っていける)が欲しい場合。ただしUIとタブレット入力の自作コストが高く、個人開発の初速は落ちる。

**避けたほうがよい**: Tauri(Windows専用なら可だが、macOS/Linux展開を考えるとWebGPUサポートが不揃い)、Godot(水彩シミュレーション実績が乏しい)。
