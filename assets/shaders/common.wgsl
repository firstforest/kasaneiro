// common.wgsl — 全シェーダー共通の定義。Rust 側(gpu/mod.rs)が各 .wgsl の先頭に連結してコンパイルする。
// このファイルは実行時ロード(H1)。保存するとアプリ再起動なしで反映される。
//
// シミュレーションテクスチャのレイアウト(いずれも rgba32float、ping-pong 2枚組):
//   水:       r = 水量 / g = 速度x / b = 速度y / a = 濡れマスク(0=乾いた紙 / 1=濡れた領域)
//   浮遊顔料: rgba の各チャンネル = 顔料1種(M1c から4顔料 = src/pigment.rs の PIGMENTS。水の流れに乗って移流する)
//   沈着顔料: 同上(紙に定着した分。移流しない)
// これに加えて紙ハイト(M1d): r32float 1枚、r = 高さ 0..1(0=谷 / 1=山)。
// CPU(src/paper.rs)が起動時に生成する静的テクスチャで、ping-pong しない。
// compute パスの binding は全シェーダー共通:
//   0/1 = 水 src/dst, 2/3 = 浮遊 src/dst, 4/5 = 沈着 src/dst, 6 = params, 7 = splats, 8 = 紙ハイト
//   9 = 顔料個性(M3): array<vec4f, 4>、各 vec4 = (密度 ρ, ステイニング ω, 粒状感 γ, 予備)。
//       起動時に1回だけ書く静的 uniform(src/pigment.rs の physics_uniform)。splat/transfer が読む
//   10 = 清書ペンの線画テクスチャ(M4.5b、r32float、read only)。ペン濃度から透水率を出して
//        velocity(速度場・にじみ拡張)/ diffuse(拡散流束)が水の境界として使う。線画パスが書く
// dst は毎パス全テクセルを書くこと(変更しないテクスチャも素通しで write する。ping-pong のため)
// 乾燥レイヤー(M2): rgba32float の texture array(1 スライス = 1 乾燥レイヤー、rgba = 4顔料濃度)。
// bake.wgsl だけが binding 9 に書き込みスライスを持つ(splats は持たない)。表示側の合成は display.wgsl 参照

// src/sim/mod.rs の SimParams と同レイアウトにすること(H2)
struct SimParams {
    brush_radius: f32,
    brush_water: f32,
    brush_velocity: f32,
    dt: f32,
    accel: f32,
    damping: f32,
    xi: f32,
    relax_iters: u32,
    vel_max: f32,
    display_mode: u32,
    display_gain: f32,
    wet_expand: f32,
    brush_pigment: f32,
    deposit_rate: f32,
    lift_rate: f32,
    evap_rate: f32,
    pigment_diffuse: f32,
    diffuse_iters: u32,
    brush_channel: u32,
    pigment_density: f32,
    paper_amp: f32,
    paper_gran: f32,
    paper_wet: f32,
    edge_eta: f32,
    edge_radius: u32,
    pressure_radius: f32,
    pressure_water: f32,
    pressure_pigment: f32,
    pressure_gamma: f32,
    dry_shift: f32,
    dry_gran: f32,
    dry_edge: f32,
    rewet_water: f32,
    tool: u32,           // M3: 0=描画 / 1=リフト(削り) / 2=消去 / 3=水筆(M4) / 4=ならし(M4)
    lift_strength: f32,  // M3: リフトの基準強度
    compose_mode: u32,   // M3: 0=multiply / 1=KM(R/T 光学合成)
    water_lift: f32,     // 水筆(tool=3): ブラシ下の顔料を近傍平均へ均す緩和率(半径2固定)
    smear_rate: f32,     // ならし(tool=4): 総顔料をブラシスケールで均す緩和率(濃い山を周囲へ伸ばす)
    line_mode: u32,        // M4.5a: ラスタ線画 0=鉛筆 / 1=ペン / 2=ハイライト(予約)
    line_eraser: u32,      // M4.5a: ラスタ消しゴム 0=描画 / 1=減算
    pencil_radius: f32,    // M4.5a: 鉛筆の半径(brush_radius と独立)
    pencil_strength: f32,  // M4.5a: 鉛筆のインク濃度基準(筆圧で変調)
    pencil_gran: f32,      // M4.5a: 鉛筆の粒状感(紙ハイトでの濃度変調度)
    pen_radius: f32,       // M4.5a: ペンの半径(brush_radius と独立)
    pen_strength: f32,     // M4.5a: ペンのインク濃度基準
    show_pencil: u32,      // M4.5a: 下書き(鉛筆)レイヤーの表示 0/1
    show_pen: u32,         // M4.5a: 清書(ペン)レイヤーの表示 0/1
    line_block: f32,       // M4.5b: ペン線の透水率の効き(perm = 1 − line_block×ペン濃度)
    highlight_radius: f32, // M4.5c: ハイライト(不透明白)の半径
    highlight_strength: f32, // M4.5c: ハイライトの不透明度基準
    show_highlight: u32,   // M4.5c: ハイライトレイヤーの表示 0/1
    active_tiles: u32,     // M6: アクティブタイル最適化 0=全面計算 / 1=濡れ+ブラシのタイルだけ計算
    paint_pickup: f32,      // 色の溶かし戻し: 筆が触れた沈着顔料を浮遊層へ戻す割合(0=無効)
    paint_spread: f32,      // 広がり: 水の足場(min(値,1))+置いた点から外向きの流れ(×筆圧×水量。0=無効)
    brush_charge: f32,      // 筆の含み(秒): 置いたまま色水が流れ出続ける時間(CPU 側で消費。0=無効)
    _pad_paint: f32,        // 16B 整列の予備(次のパラメータ追加時はここを置き換える)
};

// アクティブタイル(M6): シミュレーションを「濡れているタイル+ブラシ近傍」だけに絞る土台。
// tilescan がタイルごとに濡れ/顔料/ブラシ有無を判定し、tiledilate が1タイル分ふくらませて
// active フラグ(binding 11 = array<u32>)を作る。各シミュパスは非アクティブなタイルを素通しする。
// TILE_SIZE(1タイルの1辺テクセル数)と TILES_PER_SIDE(キャンバス1辺のタイル数)は
// キャンバスサイズ依存(M8)のため、このファイルではなく Rust 側(gpu/mod.rs の
// shader_prelude)が const 2行を生成して本ファイルのさらに前に連結する。

// テクセル座標 → タイルの線形 index(active 配列の添字)
fn tile_index_of(p: vec2u) -> u32 {
    return (p.y / TILE_SIZE) * TILES_PER_SIDE + (p.x / TILE_SIZE);
}

// 筆圧 0..1 → 応答カーブ γ を通した補間係数(M1.5)。
// 実効値 = 基準値 × mix(1, 筆圧^γ, 効き)。マウス(筆圧 1.0)は常に 1
fn pressure_curve(pressure: f32) -> f32 {
    return pow(clamp(pressure, 0.0, 1.0), max(params.pressure_gamma, 0.01));
}

// src/sim/mod.rs の Splat と同レイアウト(32 バイト)
struct Splat {
    pos: vec2f,      // テクセル座標
    vel: vec2f,      // ストローク方向の単位ベクトル(初速の向き)
    pressure: f32,   // 筆圧(マウスは 1.0)
    feed: f32,       // 筆の含み(feed)フラグ: 1 = 置いたまま毎フレーム注がれる色水(splat.wgsl 参照)
    _pad1: f32,
    _pad2: f32,
};

struct SplatBuffer {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    splats: array<Splat>,
};

// 濡れ判定(a チャンネル = Curtis 1997 の wet-area mask)。
// 水が動くのは濡れたセルだけ。乾いた紙との境界はキャンバス端と同じ「壁」として扱う。
fn is_wet(cell: vec4f) -> bool {
    return cell.a > 0.5;
}

// 境界をはみ出さない textureLoad(端のセルをそのまま延長)
fn load_clamped(t: texture_2d<f32>, p: vec2i) -> vec4f {
    let dims = vec2i(textureDimensions(t));
    return textureLoad(t, clamp(p, vec2i(0), dims - 1), 0);
}

// 手動バイリニア補間(rgba32float はサンプラーでフィルタできないため)
fn load_bilinear(t: texture_2d<f32>, p: vec2f) -> vec4f {
    let q = p - 0.5;
    let base = floor(q);
    let f = q - base;
    let i = vec2i(base);
    let c00 = load_clamped(t, i);
    let c10 = load_clamped(t, i + vec2i(1, 0));
    let c01 = load_clamped(t, i + vec2i(0, 1));
    let c11 = load_clamped(t, i + vec2i(1, 1));
    return mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
}
