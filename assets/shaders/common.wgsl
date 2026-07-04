// common.wgsl — 全シェーダー共通の定義。Rust 側(gpu/mod.rs)が各 .wgsl の先頭に連結してコンパイルする。
// このファイルは実行時ロード(H1)。保存するとアプリ再起動なしで反映される。
//
// シミュレーションテクスチャのレイアウト(いずれも rgba32float、ping-pong 2枚組):
//   水:       r = 水量 / g = 速度x / b = 速度y / a = 濡れマスク(0=乾いた紙 / 1=濡れた領域)
//   浮遊顔料: rgba の各チャンネル = 顔料1種(M1b は r のみ。水の流れに乗って移流する)
//   沈着顔料: 同上(紙に定着した分。移流しない)
// compute パスの binding は全シェーダー共通:
//   0/1 = 水 src/dst, 2/3 = 浮遊 src/dst, 4/5 = 沈着 src/dst, 6 = params, 7 = splats
// dst は毎パス全テクセルを書くこと(変更しないテクスチャも素通しで write する。ping-pong のため)

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
};

// src/sim/mod.rs の Splat と同レイアウト(32 バイト)
struct Splat {
    pos: vec2f,      // テクセル座標
    vel: vec2f,      // ストローク方向の単位ベクトル(初速の向き)
    pressure: f32,   // 筆圧(マウスは 1.0)
    _pad0: f32,
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
