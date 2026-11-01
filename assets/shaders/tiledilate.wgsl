// tiledilate.wgsl — アクティブタイル(M6)の第2段: raw_active を1タイル分ふくらませて active を作る。
// 1 invocation = 1タイル。自分と8近傍のどれかが有効なら自分も有効にする(3×3 の膨張)。
// これで濡れ前線が1フレームに TILE_SIZE テクセル進んでも、前線の外側タイルが先に有効化されており、
// gate で止められて凍りつくことがない(vel_max × sim_steps < TILE_SIZE の範囲で安全)。
//
// 計算しないタイルは、ping-pong の2枚が一致しているなら書き込みも省く(TILE_COLD)。
// 一致を保証するのが dirty: 計算したタイルはそのフレームの最後のパスで2枚が食い違うので
// dirty=1 を残し、次にシミュが進むフレームで計算しなければ TILE_COPY として全パスでコピーされ揃う。
// このパスはシミュが少なくとも1パス進むフレームにだけ走る(gpu/callback.rs)ので、
// 「今フレーム計算するタイル = 今フレームの終わりに食い違うタイル」がそのまま次の dirty になる。
// 先頭に common.wgsl が連結される(TILES_PER_SIDE / TILE_COLD / TILE_COPY / TILE_ACTIVE)。

@group(0) @binding(0) var<storage, read> raw_active: array<u32>;
@group(0) @binding(1) var<storage, read_write> active_out: array<u32>;
// common.wgsl の pressure_curve が参照するため params を束ねる(このパスでは使わない)
@group(0) @binding(2) var<uniform> params: SimParams;
@group(0) @binding(3) var<storage, read_write> dirty: array<u32>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= TILES_PER_SIDE || gid.y >= TILES_PER_SIDE) {
        return;
    }
    var a = 0u;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let nx = i32(gid.x) + dx;
            let ny = i32(gid.y) + dy;
            if (nx < 0 || ny < 0 || nx >= i32(TILES_PER_SIDE) || ny >= i32(TILES_PER_SIDE)) {
                continue;
            }
            if (raw_active[u32(ny) * TILES_PER_SIDE + u32(nx)] != 0u) {
                a = 1u;
            }
        }
    }
    let idx = gid.y * TILES_PER_SIDE + gid.x;
    if (a != 0u) {
        active_out[idx] = TILE_ACTIVE;
    } else if (dirty[idx] != 0u) {
        active_out[idx] = TILE_COPY;
    } else {
        active_out[idx] = TILE_COLD;
    }
    dirty[idx] = a;
}
