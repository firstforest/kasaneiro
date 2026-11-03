// common.wgsl — 全シェーダー共通の定義。Rust 側(gpu/mod.rs)が各 .wgsl の先頭に連結してコンパイルする。
// このファイルは実行時ロード(H1)。保存するとアプリ再起動なしで反映される。
//
// シミュレーションテクスチャのレイアウト(いずれも rgba32float、ping-pong 2枚組):
//   水:       r = 水量 / g = 速度x / b = 速度y / a = 濡れマスク(0=乾いた紙 / 1=濡れた領域)
//   浮遊顔料: rgba の各チャンネル = 顔料1種(パレットの4スロット = crates/pigment/src/lib.rs の Palette。水の流れに乗って移流する)
//   沈着顔料: 同上(紙に定着した分。移流しない)
// これに加えて紙ハイト(M1d): r32float 1枚、r = 高さ 0..1(0=谷 / 1=山)。
// CPU(crates/paint-core/src/paper.rs)が起動時に生成する静的テクスチャで、ping-pong しない。
// compute パスの binding は全シェーダー共通:
//   0/1 = 水 src/dst, 2/3 = 浮遊 src/dst, 4/5 = 沈着 src/dst, 6 = params, 7 = splats, 8 = 紙ハイト
//   9 = 顔料個性(M3): array<vec4f, 4>、各 vec4 = (密度 ρ, ステイニング ω, 粒状感 γ, 粒の細かさ μ)。
//       パレット編集で書き替わる uniform(crates/pigment の physics_uniform)。
//       splat(ω/γ)/ transfer(ρ/ω/γ)/ diffuse(μ=分離色のチャンネル別拡散)が読む
//   10 = 清書ペンの線画テクスチャ(M4.5b、r32float、read only)。ペン濃度から透水率を出して
//        velocity(速度場・にじみ拡張)/ diffuse(拡散流束)が水の境界として使う。線画パスが書く
//   11 = アクティブタイルの状態(M6)
//   12 = 塗る筆の中身(Brush、穂先と腹の2室、read_write)。splat が穂先を読んで受け渡しを決め、
//        brush.wgsl がフレームの最後に更新する
//   13 = 穂先と紙が受け渡した量の集計(BrushFlow、atomic)。splat が足し、brush.wgsl が穂先へ移して 0 に戻す
// dst は計算するタイル(TILE_ACTIVE)とコピーするタイル(TILE_COPY)の全テクセルを書くこと
// (変更しないテクスチャも素通しで write する。ping-pong のため)。TILE_COLD のタイルは2枚が一致しているので書かない
// 乾燥レイヤー(M2): rgba32float の texture array(1 スライス = 1 乾燥レイヤー、rgba = 4顔料濃度)。
// bake.wgsl だけが binding 9 に書き込みスライスを持つ(splats は持たない)。表示側の合成は display.wgsl 参照

// crates/paint-core/src/sim.rs の SimParams と同レイアウトにすること(H2。tests/sim_params_layout.rs が検査)
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
    tool: u32,           // 0=描画 / 1=リフト(削り) / 2=消去 / 3=吸い取り
    lift_strength: f32,  // M3: リフトの基準強度
    compose_mode: u32,   // M3: 0=multiply / 1=KM(R/T 光学合成)
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
    brush_exchange: f32,    // 筆と紙の受け渡しの速さ: 1 splat で濡れ具合の差 × 満タン水位のこの割合だけ水が動く
    absorb_rate: f32,       // 吸い取り(tool=3): 1 splat で自由水+浮遊顔料を取り除く割合(沈着は残る)
    brush_hold: f32,        // 筆の腹の大きさ(px): 満タンの腹だけで乾いた紙をこの長さ塗ると半分になる(0=減らない筆)
    lift_blot: f32,         // 削り(tool=1)の吸い取り: 浮かせた色ごと水を取り去る割合(0=浮かせるだけ)
    brush_tip: f32,         // 筆の穂先の大きさ(px): 満タンの穂先だけで乾いた紙をこの長さ塗ると半分になる
    brush_refill_time: f32, // 腹から穂先へ送る時定数(秒)。一方向
    paper_drift: f32,       // 紙の谷への寄り: 浮遊顔料が紙の谷へ集まる強さ(顔料の γ と乗算)
    mix_mottle: f32,        // 混ざりムラ: 浮遊顔料が数十 px 規模の斑へ集まる強さ(全顔料)
    advect_sharp: f32,      // 移流の MacCormack 補正の割合(0=セミラグランジアン)
    wick_bare: f32,         // 白い紙(沈着顔料なし)でのにじみ拡張の倍率(0=白い紙へは広がらない)
    wick_ref: f32,          // にじみ拡張が wet_expand そのままになる沈着量(4ch の和)
    edge_aa: f32,           // 色の縁のアンチエイリアスの強さ(0=階段のまま。edge_aa_conc)
    pen_smooth: f32,        // 清書ペンの手ぶれ補正(CPU 側で使う。シェーダーは読まない)
    pencil_eraser_radius: f32,    // 鉛筆レイヤーの消しゴムの半径(描く太さとは別)
    pen_eraser_radius: f32,       // 清書ペンレイヤーの消しゴムの半径
    highlight_eraser_radius: f32, // ハイライトレイヤーの消しゴムの半径
    pen_smooth_speed: f32,  // 清書ペンの手ぶれ補正を速さで弱める強さ(CPU 側で使う)
    pen_tail: f32,          // 清書ペンの抜き(CPU 側で使う)
};

// アクティブタイル(M6): シミュレーションを「濡れているタイル+ブラシ近傍」だけに絞る土台。
// tilescan がタイルごとに濡れ/ブラシ有無を判定し、tiledilate が1タイル分ふくらませて
// タイルの状態(binding 11 = array<u32>。TILE_COLD / TILE_COPY / TILE_ACTIVE)を作る。
// 各シミュパスは ACTIVE だけ計算し、COPY は素通しでコピー、COLD は何も書かない。
// TILE_SIZE(1タイルの1辺テクセル数)・TILES_PER_SIDE(キャンバス1辺のタイル数。M8 で
// キャンバスサイズ依存)をはじめ Rust と共有する定数(MAX_LAYERS / LATENT_TOTAL など)は、
// このファイルではなく Rust 側(src/gpu/prelude.rs の shader_prelude)が const として生成し、
// 本ファイルのさらに前に連結する。

// テクセル座標 → タイルの線形 index(active 配列の添字)
fn tile_index_of(p: vec2u) -> u32 {
    return (p.y / TILE_SIZE) * TILES_PER_SIDE + (p.x / TILE_SIZE);
}

// 筆圧 0..1 → 応答カーブ γ を通した補間係数(M1.5)。
// 実効値 = 基準値 × mix(1, 筆圧^γ, 効き)。マウス(筆圧 1.0)は常に 1
fn pressure_curve(pressure: f32) -> f32 {
    return pow(clamp(pressure, 0.0, 1.0), max(params.pressure_gamma, 0.01));
}

// crates/paint-core/src/sim.rs の Splat と同レイアウト(32 バイト)
struct Splat {
    pos: vec2f,      // テクセル座標
    vel: vec2f,      // ストローク方向の単位ベクトル(初速の向き)
    pressure: f32,   // 筆圧(マウスは 1.0)
    feed: f32,       // 置いたまま(feed)フラグ: 1 = 塗る筆を置いたまま動かない間に毎フレーム積む splat(splat.wgsl 参照)
    scale: f32,      // 穂先の中身のうち、この splat を置く時点で残っている割合の予測(フレーム頭の中身に対する比。乾いた紙に置き補充も無い場合 = 最も減った場合)
    _pad: f32,
};

struct SplatBuffer {
    count: u32,        // splat 数(splat / linesplat が読む)
    fluid_count: u32,  // 流体へ流す splat 数(tilescan が読む。線画ツールのフレームは 0)
    brush_dt: f32,     // brush.wgsl が腹から穂先へ補充する時間(秒。前回の brush パスからの経過)
    _pad: u32,
    splats: array<Splat>,
};

// 塗る筆の中身(crates/paint-core/src/load.rs の BrushState と同レイアウト、64 バイト)。
// 穂先(紙と受け渡す)と腹(穂先へ水を送る)の2室で、どちらも水 1 = その室が満タン、顔料 1 = 標準の1回分
struct Brush {
    tip_water: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    tip_pigment: vec4f,
    belly_water: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
    belly_pigment: vec4f,
};

// 穂先と紙が受け渡した量の集計(1フレーム分、穂先へ入った向きが正)。WGSL に float の atomic が無いので
// 固定小数点(× FLOW_SCALE)の i32 で足す。[0] = 水、[1..4] = 4スロットの顔料(どれもセルの量の合計)
struct BrushFlow {
    amount: array<atomic<i32>, 5>,
};

// 受け渡した量の固定小数点の刻み。1セル1フレームの受け渡しは満タン水位(2 前後)までなので、
// 64px の筆で 1024² のキャンバスを1フレームで横切っても i32 に収まる
const FLOW_SCALE: f32 = 4096.0;

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

// 縁のアンチエイリアス(edge_aa): 濡れマスクが二値なので、顔料の縁は1テクセル単位の階段になる。
// 「色がある/ない」を3×3のテント重みでならした場 B と、その勾配(Sobel)から縁(B=0.5 の等値線)までの
// 符号つき距離を出し、テクセルを覆う割合 cov = clamp(距離 + 0.5, 0, 1) にする。色のあるテクセルは cov で
// 薄め、色のないテクセルは隣の色のあるテクセルの濃さを cov だけ借りる。グリッドに沿った真っ直ぐな縁は
// cov が 1 / 0 になるので変わらず、階段の角だけがなめらかになる。
// c[(dy+1)*3 + (dx+1)] = 近傍テクセルの顔料濃度(浮遊+沈着)。strength = params.edge_aa(0 で素通し)
const EDGE_AA_PRESENCE: f32 = 0.002; // density×総濃度(≒被覆率)がこれを超えるテクセルを「色がある」とする
fn edge_aa_conc(c_in: array<vec4f, 9>, density: f32, strength: f32) -> vec4f {
    var c = c_in;
    let center = max(c[4], vec4f(0.0));
    if (strength <= 0.0) {
        return center;
    }
    var pres: array<f32, 9>;
    for (var i = 0u; i < 9u; i++) {
        c[i] = max(c[i], vec4f(0.0));
        pres[i] = select(0.0, 1.0, density * dot(c[i], vec4f(1.0)) > EDGE_AA_PRESENCE);
    }
    let b = (pres[0] + pres[2] + pres[6] + pres[8]
        + 2.0 * (pres[1] + pres[3] + pres[5] + pres[7]) + 4.0 * pres[4]) / 16.0;
    let gx = (pres[2] + 2.0 * pres[5] + pres[8] - pres[0] - 2.0 * pres[3] - pres[6]) / 8.0;
    let gy = (pres[6] + 2.0 * pres[7] + pres[8] - pres[0] - 2.0 * pres[1] - pres[2]) / 8.0;
    let g = length(vec2f(gx, gy));
    if (g < 1e-4) {
        return center;
    }
    let cov = clamp((b - 0.5) / g + 0.5, 0.0, 1.0);
    if (pres[4] > 0.5) {
        return center * mix(1.0, cov, strength);
    }
    var w = array<f32, 9>(1.0, 2.0, 1.0, 2.0, 0.0, 2.0, 1.0, 2.0, 1.0);
    var sum = vec4f(0.0);
    var wsum = 0.0;
    for (var i = 0u; i < 9u; i++) {
        sum += (w[i] * pres[i]) * c[i];
        wsum += w[i] * pres[i];
    }
    if (wsum <= 0.0) {
        return center;
    }
    return center + (sum / wsum - center) * (cov * strength);
}
