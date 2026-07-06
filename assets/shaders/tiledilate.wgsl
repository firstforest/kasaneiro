// tiledilate.wgsl — アクティブタイル(M6)の第2段: raw_active を1タイル分ふくらませて active を作る。
// 1 invocation = 1タイル。自分と8近傍のどれかが有効なら自分も有効にする(3×3 の膨張)。
// これで濡れ前線が1フレームに TILE_SIZE テクセル進んでも、前線の外側タイルが先に有効化されており、
// gate で素通しされて凍りつくことがない(vel_max × sim_steps < TILE_SIZE の範囲で安全)。
// 先頭に common.wgsl が連結される(TILES_PER_SIDE)。

@group(0) @binding(0) var<storage, read> raw_active: array<u32>;
@group(0) @binding(1) var<storage, read_write> active_out: array<u32>;
// common.wgsl の pressure_curve が参照するため params を束ねる(このパスでは使わない)
@group(0) @binding(2) var<uniform> params: SimParams;

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
    active_out[gid.y * TILES_PER_SIDE + gid.x] = a;
}
