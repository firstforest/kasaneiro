// splat.wgsl — ブラシで「水を置く」compute パス(M1a: 水量+初速の注入)。
// 先頭に common.wgsl が連結される(SimParams / Splat / ヘルパー関数はそちら)。

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> params: SimParams;
@group(0) @binding(3) var<storage, read> splat_buf: SplatBuffer;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_tex);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }

    let p = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let cell = textureLoad(src_tex, vec2i(gid.xy), 0);
    var water = cell.r;
    var vel = cell.gb;
    var wet = cell.a;

    for (var i = 0u; i < splat_buf.count; i++) {
        let s = splat_buf.splats[i];
        let radius = max(params.brush_radius * s.pressure, 0.5);
        let dist = distance(p, s.pos);
        // 中心は満量、縁にかけて柔らかく減衰
        let coverage = 1.0 - smoothstep(radius * 0.6, radius, dist);
        water += coverage * params.brush_water;
        vel += coverage * params.brush_velocity * s.vel;
        // 筆が届いた範囲を濡らす(wet-area mask)。水が動けるのはこの領域だけ。
        // 水を置く範囲(coverage > 0)と一致させ、マスク外に水が取り残されないようにする
        if (dist < radius) {
            wet = 1.0;
        }
    }

    // 安定性: 水量は非負、速度は CFL 的上限でクランプ
    water = max(water, 0.0);
    let speed = length(vel);
    if (speed > params.vel_max) {
        vel *= params.vel_max / speed;
    }

    textureStore(dst_tex, vec2i(gid.xy), vec4f(water, vel, wet));
}
