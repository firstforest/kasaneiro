// flowout.wgsl — FlowOutward パス(M1d)。Curtis 1997 のエッジダークニング。
// 濡れマスク M をぼかした M' を使い、濡れ領域の縁に近いセルほど水を除去する:
//   p ← p − η·(1−M')·M
// 縁で水が減る → 水面勾配が内側 → 縁向きになり、次の速度更新・移流が浮遊顔料を
// 縁へ運んで溜める(コーヒーリング効果)。顔料はこのパスでは動かさない。
// M' はボックスぼかし(半径 edge_radius、1..8 テクセル)。厳密なガウシアンである必要はない。
// edge_eta = 0 のときは CPU 側(gpu/mod.rs)がこのパスの dispatch 自体を省略する。
// 先頭に common.wgsl が連結される。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var dst_water: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var dst_susp: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var src_dep: texture_2d<f32>;
@group(0) @binding(5) var dst_dep: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;
@group(0) @binding(8) var paper_tex: texture_2d<f32>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_water);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_water, ip, 0);

    // 顔料は素通し(ping-pong のため必ず書く)
    textureStore(dst_susp, ip, textureLoad(src_susp, ip, 0));
    textureStore(dst_dep, ip, textureLoad(src_dep, ip, 0));

    // 乾いたセルは素通し(M=0 なので除去量も 0)
    if (!is_wet(cell)) {
        textureStore(dst_water, ip, cell);
        return;
    }

    // M' = 濡れマスクのボックスぼかし。内部深くでは 1(除去なし)、縁では < 1
    let radius = i32(clamp(params.edge_radius, 1u, 8u));
    var sum = 0.0;
    for (var dy = -radius; dy <= radius; dy++) {
        for (var dx = -radius; dx <= radius; dx++) {
            let n = load_clamped(src_water, ip + vec2i(dx, dy));
            sum += select(0.0, 1.0, is_wet(n));
        }
    }
    let side = f32(2 * radius + 1);
    let m_blur = sum / (side * side);

    let water = max(cell.r - params.edge_eta * params.dt * (1.0 - m_blur), 0.0);
    textureStore(dst_water, ip, vec4f(water, cell.gb, cell.a));
}
