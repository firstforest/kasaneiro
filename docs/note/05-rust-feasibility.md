# Rust での実現可能性評価

「Rust でこの水彩ペイントツールを作れるか」の調査ノート。調査日: 2026-07-03。

関連ノート: [00-overview.md](00-overview.md) / [01-fluid-simulation.md](01-fluid-simulation.md) / [02-pigment-mixing.md](02-pigment-mixing.md) / [03-layering-glazing-lifting.md](03-layering-glazing-lifting.md) / [04-implementation-stack.md](04-implementation-stack.md)

---

## 結論: 実現可能。しかも本プロジェクトとの相性は良い

**Rust + wgpu で全要件が実現できる。** 理由は単純で、このプロジェクトの技術的核心(Curtis 1997 系の流体シミュレーション、KM 混色、レイヤー合成)はすべて **WGSL compute shader のコード**であり、それは TypeScript + WebGPU 案と Rust + wgpu 案で**一字一句同じものが動く**からだ。wgpu は WebGPU API の Rust 実装そのもの([wgpu.rs](https://wgpu.rs/) / [gfx-rs/wgpu](https://github.com/gfx-rs/wgpu))であり、Firefox の WebGPU 実装のバックエンドでもある。つまり [04](04-implementation-stack.md) で立てたシミュレーション設計(テクスチャ構成・ping-pong・アクティブタイル)は**プラットフォーム選択と独立に成立**する。

Rust 選択で変わるのは「シェル」側 — ウィンドウ、ペン入力、UI、配布 — であり、2021〜2023 年頃はここが Web 系に大きく劣っていた。しかし 2025〜2026 年時点では下記の通り、個人開発ツールとして十分な水準に達している。[04](04-implementation-stack.md) §3-3 に書いた「WinTab 対応を自前実装する必要があり、これは Web 系を選ぶ大きな理由になる」という評価は、**winit 0.31 と octotablet の登場により古くなった**(§2 参照)。

| 懸念だった点 | 2026年時点の状況 | 深刻度 |
|---|---|---|
| GPU compute | wgpu で WebGPU と同一の WGSL がネイティブ実行できる | 解決済み |
| ペン入力(筆圧) | winit 0.31 の Pointer 刷新でペン対応がマージ済み + octotablet | ほぼ解決(§2) |
| UI(カラーピッカー・レイヤーパネル) | egui / eframe で実用水準。キャンバスは wgpu パスを直接埋め込める | 解決済み(§3) |
| 混色ライブラリ | Mixbox に公式 Rust クレートあり。スペクトラル自作も容易 | 解決済み(§4) |
| 開発初速 | HTML/CSS より遅い。これは残るコスト | 残る(§7) |

---

## 1. GPU 基盤: wgpu

- **wgpu は WebGPU API 準拠の Rust ライブラリ**。ネイティブでは Vulkan / DirectX 12 / Metal / OpenGL ES バックエンドで動き、`wasm32` ターゲットではブラウザの WebGPU に直接マップされる([wgpu.rs](https://wgpu.rs/))
- シェーダー言語は WGSL(内部のシェーダー変換系は [naga](https://github.com/gfx-rs/wgpu/tree/trunk/naga))。**[04](04-implementation-stack.md) の推奨構成「WGSL compute で Curtis 簡略版」はそのまま流用できる** — compute pipeline、storage texture、ping-pong、atomics すべて WebGPU と同一 API 概念
- ブラウザ由来の制約(タブごとの GPU メモリ制限、readback の非同期強制、タイマー精度制限)が**ネイティブでは存在しない**。特にキャンバスの保存・undo 用のテクスチャ readback はネイティブのほうが素直に書ける
- 流体シミュレーションの実績: [Wumpf/blub](https://github.com/Wumpf/blub)(wgpu で PIC/FLIP/APIC 3D 流体 — 本プロジェクトの 2D グリッドよりはるかに重い計算が動いている実証)、[lisyarus/webgpu-shallow-water](https://github.com/lisyarus/webgpu-shallow-water)(浅水方程式、WebGPU API)
- 注意: wgpu は開発が活発でメジャーバージョンが年数回上がり、**wgpu / winit / egui の3者はバージョンをロックステップで上げる**必要がある(egui の対応 wgpu バージョンに合わせるのが楽)

**評価: リスクなし。** 本プロジェクトの計算核心は wgpu の最も枯れた使い方(2D テクスチャの compute 更新)しか使わない。

## 2. ペン入力(筆圧・傾き)— かつての最大の懸念、現在はほぼ解決

Rust ネイティブの弱点とされてきた領域だが、2024〜2025 年に状況が変わった。

### winit 0.31 の Pointer Event 刷新

- winit は [Pointer Event Overhaul(Issue #3833)](https://github.com/rust-windowing/winit/issues/3833) で入力系を Web の Pointer Events 相当に再設計し、**0.31 系でマウス・タッチ・ペンを統一 Pointer イベントに集約、ペン入力サポート(Windows / Wayland / Web)がマージされた**([v0.31.0-beta.1 リリースノート](https://github.com/rust-windowing/winit/releases/tag/v0.31.0-beta.1))
- それ以前の経緯: [Issue #99](https://github.com/rust-windowing/winit/issues/99)(2016年からの要望)、[PR #2396](https://github.com/rust-windowing/winit/pull/2396)(Windows/Android のペン+筆圧)など長年未マージだった — 「winit は筆圧が取れない」という古い評判はこの時期のもの
- 安定版に乗るまでのつなぎ、または winit のイベントで不足する場合(傾き・ホバー距離・消しゴム側検出・デバイス識別など)は次の octotablet を使う

### octotablet — 高レベルのタブレット/スタイラス API

- [Fuzzyzilla/octotablet](https://github.com/Fuzzyzilla/octotablet)([docs.rs](https://docs.rs/octotablet/latest/octotablet/))— ペイントアプリ用途を主眼に開発されている Rust クレート。**Windows では Windows Ink(RealTimeStylus)経由**で筆圧・傾き・ホバーを取得し、`raw-window-handle` 経由で **winit / eframe とそのまま統合できる**(eframe のデバッグビューア例が同梱)
- より低レベルには [wintab_lite](https://github.com/thehappycheese/wintab_lite)(WinTab の薄いバインディング)もある
- WinTab と Windows Ink の使い分けは Krita と同じ議論([Krita Manual](https://docs.krita.org/en/user_manual/drawing_tablets.html)、[解説](https://docs.thesevenpens.com/drawtab/developers/wintab-vs-windows-ink)): 現代の Wacom/XP-Pen ドライバは Windows Ink をサポートしており、**個人ツールなら Windows Ink(= octotablet / winit)一本で開始してよい**。WinTab 対応は「古いドライバ設定のユーザーも救う」段階になってから検討で十分

> **追記(2026-07-05、M1.5 実装時)**: octotablet は**不採用**になった。Manager を作成すると RTS コールバックスレッド(COM マーシャリングで UI スレッドのメッセージポンプ待ち)と UI スレッド(`pump()` の mutex でコールバック待ち)が相互待ちし、アプリが「応答なし」になる既知バグを実機で確認(crates.io 0.1.0 / git master とも再現。[issue #18](https://github.com/Fuzzyzilla/octotablet/issues/18))。実際には eframe 0.35(winit 0.30 系)の時点で winit が WM_POINTER を処理し `GetPointerPenInfo` の筆圧を `Touch{force}` として届けており、egui-winit がそれを `egui::Event::Touch{force}` に変換するため、**筆圧だけなら追加クレートなしで取れた**(winit 0.31 の Pointer 刷新を待つ必要もなかった)。傾き・ホバー距離・消しゴム検出が必要になったら octotablet を再評価する(ハングバグの修正確認が前提。plan.md §4)。

### Web 版 Pointer Events との対応関係

| Web ([04](04-implementation-stack.md) §3-3) | Rust での対応物 |
|---|---|
| `PointerEvent.pressure` / `tiltX/Y` | winit 0.31 Pointer / octotablet の軸データ |
| `getCoalescedEvents()`(中間サンプル) | ネイティブではイベントが OS レートでそのまま届くため**そもそも間引きが起きない**(Windows Ink は 133Hz+) |
| `pointerrawupdate`(低遅延) | 同上。むしろネイティブのほうが遅延面で有利 |

**評価: 小リスク。** winit 0.31 が正式安定化する前に始めるなら octotablet 併用が安全弁になる。最初の Phase 1(マウスで splat 確認)には何も要らない。

## 3. UI: egui / eframe

- [egui](https://github.com/emilk/egui) は Rust の即時モード GUI。スライダー・カラーピッカー・ドッキングパネルなど、**ペイントツールの「周辺 UI」に必要な部品は標準またはエコシステムで揃う**。eframe(公式フレームワーク)+ [egui-wgpu](https://docs.rs/egui-wgpu) で wgpu レンダラを共有できる
- キャンバス部分は egui の **`PaintCallback`** で任意の wgpu レンダーパスを UI 内に直接埋め込める — 「シミュレーション結果のテクスチャを egui ウィンドウ内に表示し、周囲に HTML ライクなパネルを置く」という [00](00-overview.md) のアーキテクチャ図がそのまま組める
- 代替: [iced](https://github.com/iced-rs/iced)(Elm 風・保持モード)、[Slint](https://slint.dev/)(宣言的)。ツールパレット程度の UI なら egui が最速で、実験的パラメータ調整 UI(スライダー山盛りのデバッグパネル)を作る文化とも相性が良い — **流体シミュのパラメータ調整が品質の本体**([01](01-fluid-simulation.md) §1)である本プロジェクトでは、これは小さくない利点
- HTML/CSS 比の劣位は残る: 凝ったビジュアルデザイン、テキスト処理、アクセシビリティ。個人用ツールでは許容範囲

**評価: リスクなし(デザイン性への期待値を正しく持てば)。**

## 4. 混色: Mixbox 公式 Rust クレート + 自作スペクトラル

- **[mixbox クレート](https://crates.io/crates/mixbox)が公式に存在する**([lib.rs](https://lib.rs/crates/mixbox)、[scrtwpns/mixbox](https://github.com/scrtwpns/mixbox) リポジトリに Rust 実装同梱)。`mixbox::lerp` / `rgb_to_latent` / `latent_to_rgb` で [02](02-pigment-mixing.md) の設計そのまま。ライセンスは同じく **CC BY-NC 4.0(非商用)**。ラッパー [pigment-mixing-rs](https://github.com/virtualritz/pigment-mixing-rs) もある
- GPU 側の混色は Mixbox 同梱の GLSL シェーダーを WGSL に移植(LUT テクスチャ1枚+多項式評価のみなので機械的な作業)
- ライセンスフリー路線([02](02-pigment-mixing.md) §3 の spectral.js / WGM 方式)は**そもそも数式を自前実装する方針なので言語を問わない**。むしろ CPU 参照実装(テスト用)とWGSL 実装を並走させて突き合わせる開発スタイルは、cargo test が回しやすい Rust の得意分野
- KM 層合成([03](03-layering-glazing-lifting.md) §3)も同様に数式の直接実装であり、言語依存なし

**評価: リスクなし。**

## 5. 先行事例(Rust でグラフィックツールは作れているか)

- **[Graphite](https://graphite.rs/)**([GitHub](https://github.com/GraphiteEditor/Graphite))— Rust 製の 2D グラフィックエディタ。規模の大きい実例で、「Rust でグラフィックツールのフルアプリを作り切れる」ことの実証
- **octotablet の作者自身がペイントアプリを開発中**であり、クレート群(タブレット入力)がその副産物として整備されている — ニッチだが「Rust でペイントアプリ」の同時代の動きは存在する
- Krita / Rebelle 級の完成品はまだ Rust には存在しない。ただし本プロジェクトは商用完成度の汎用ペイントツールではなく**水彩シミュレーション特化の自分用ツール**なので、先行完成品の不在は障害ではない

## 6. 将来の Web 展開の道が残る(Rust 選択の隠れた利点)

- wgpu + egui/eframe は `wasm32-unknown-unknown` でビルドすればブラウザでも動く(eframe は公式に Web 対応、wgpu は WebGPU バックエンド)。**「ネイティブで開発して、配りたくなったら同一コードベースで Web 版」という順路が取れる**
- これは 04 の第一候補(TS + WebGPU)の逆方向: TS 案は「Web で開発して Electron で包む」、Rust 案は「ネイティブで開発して wasm で開く」。**どちらも WGSL 資産は共通**なので、後からの路線変更でシェーダーとアルゴリズム(=価値の本体)は無傷で移る
- 注意: wasm 版ではペン入力がブラウザの Pointer Events 経由に戻る(octotablet は使えない)、ファイル保存も Web API 経由になる。入力とストレージを trait で抽象化しておくと移植が単純になる

## 7. Rust ならではの利点と、正直なコスト

### 利点

1. **CPU 側の重い処理に強い**: アクティブタイル管理、undo 履歴(テクスチャ差分の圧縮)、大判キャンバスの保存、将来の CPU フォールバック([rayon](https://crates.io/crates/rayon) で並列化)— GC 停止がなくフレーム落ちの原因が減る。ストローク中のレイテンシ安定性は描き味に直結する
2. **テスト駆動でシミュレーションを開発できる**: Curtis のセルオートマトンは CPU 参照実装(ナイーブな Rust)を先に書いて cargo test で数値検証し、その後 WGSL に移植して突き合わせる、という進め方ができる。シェーダーのデバッグ困難性への実務的な対策になる
3. **配布が単一 exe**: Electron の ~100MB+ランタイムに対し、Rust ネイティブは数 MB〜十数 MB の単一バイナリ。自分用ツールの起動の速さ・気軽さに効く
4. **wgpu の validation layer が優秀**: WebGPU 系 API はバリデーションが厳格で、素の Vulkan/D3D12 より GPU プログラミングの学習曲線が緩い

### 正直なコスト

1. **開発初速は TS + WebGPU より遅い**。特に UI の試行錯誤とホットリロード(Web の DevTools 的な体験はない。egui の即時モードが部分的に補う)
2. **winit 0.31 のペン対応は安定版になって日が浅い**(または beta)。octotablet という迂回路があるため致命的ではない
3. **wgpu / winit / egui のバージョン追従**という Rust エコシステム特有の維持コスト
4. コンパイル時間。シェーダー(WGSL)はランタイムロードにしてホットリロード可能にしておくと、**調整作業の大半(=シェーダーパラメータいじり)は再コンパイル不要**にできる — これは必須の開発環境投資

## 8. 推奨構成(Rust 版)

```
┌─ シェル: winit 0.31(Pointerイベント) + octotablet(筆圧・傾きの保険)─┐
│                                                                    │
│  UI: egui / eframe(egui-wgpuでレンダラ共有)                        │
│    キャンバス = PaintCallback で wgpu レンダーパスを直接埋め込み       │
│    パラメータ調整パネル(スライダー群)を最初から作る                   │
│                                                                    │
│  シミュレーション: WGSL compute(04 の設計そのまま・変更なし)          │
│    512²から開始 / ping-pong / アクティブタイル                       │
│    WGSLはファイルからランタイムロード+ホットリロード                   │
│                                                                    │
│  混色: mixbox クレート+LUTシェーダーWGSL移植(非商用CC BY-NC)         │
│        or 自作スペクトラルWGM(02 §3、ライセンスフリー)               │
│                                                                    │
│  CPU側: 参照実装+cargo testで数値検証 / undo履歴 / タイル管理         │
└────────────────────────────────────────────────────────────────────┘
配布: 単一exe(Windows)。将来は wasm32 + WebGPU で Web 版の道あり
```

### 主要クレート一覧

| 役割 | クレート | 備考 |
|---|---|---|
| GPU | [wgpu](https://crates.io/crates/wgpu) | WGSL compute。設計は [04](04-implementation-stack.md) §3 のまま |
| ウィンドウ・入力 | [winit](https://crates.io/crates/winit) 0.31+ | Pointer 刷新でペン対応 |
| タブレット詳細 | [octotablet](https://crates.io/crates/octotablet) | Windows Ink。傾き・ホバー・消しゴム検出 |
| UI | [egui](https://crates.io/crates/egui) + eframe + egui-wgpu | PaintCallback でキャンバス統合 |
| 混色 | [mixbox](https://crates.io/crates/mixbox) | CC BY-NC。商用化するなら自作 WGM に差し替え |
| CPU 並列 | [rayon](https://crates.io/crates/rayon) | 参照実装・保存処理など |
| 画像 I/O | [image](https://crates.io/crates/image) | PNG 書き出し等 |
| ファイルダイアログ | [rfd](https://crates.io/crates/rfd) | ネイティブダイアログ |

### ロードマップへの影響([00](00-overview.md) の Phase 構成に対する差分)

- **Phase 0 を追加**: winit + wgpu + egui の空アプリ(ウィンドウ+テクスチャ表示+スライダー1本+WGSL ホットリロード)を最初に組む。ここが TS 案には無い先行投資(数日規模)
- Phase 1 のブラシは**マウスで開始**してよい(splat の確認に筆圧は不要)。筆圧統合は Phase 1 の後半で octotablet を足す
- Phase 2 以降(乾燥・レイヤー・リフティング)は言語非依存で計画変更なし

## 9. 結論の再掲

- **実現可能性: ◎。** 技術核心(WGSL compute + KM 混色)は言語選択と無関係に成立し、Rust 側の弱点だったペン入力と UI は winit 0.31 / octotablet / egui で埋まっている
- **Rust を選ぶ積極的理由**: 単一バイナリ配布、ストローク中のレイテンシ安定性、CPU 参照実装によるテスト駆動のシミュレーション開発、将来の wasm/Web 展開の余地
- **支払うコスト**: シェル構築の先行投資(Phase 0)と UI 開発の初速。WGSL ホットリロードを最初に整備することでコストの大半(パラメータ調整の反復)は回避できる
