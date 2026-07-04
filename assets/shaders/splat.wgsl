// splat.wgsl — マウス/ペンのストローク点をキャンバスに描く compute パス。
// このファイルは実行時ロード(H1)。保存するとアプリ再起動なしで反映される。

struct SimParams {
    brush_radius: f32,
    brush_flow: f32,
    _pad: vec2f,
    brush_color: vec4f,
};

struct Splat {
    pos: vec2f,      // テクセル座標
    pressure: f32,   // 筆圧(マウスは 1.0)
    _pad: f32,
};

struct SplatBuffer {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    splats: array<Splat>,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: SimParams;
@group(0) @binding(3) var<storage, read> splat_buf: SplatBuffer;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_tex);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }

    let p = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    var color = textureLoad(src_tex, vec2i(gid.xy), 0);

    for (var i = 0u; i < splat_buf.count; i++) {
        let s = splat_buf.splats[i];
        let radius = max(params.brush_radius * s.pressure, 0.5);
        let d = distance(p, s.pos);
        // 中心は不透明、縁にかけて柔らかく減衰
        let coverage = 1.0 - smoothstep(radius * 0.6, radius, d);
        let amount = coverage * params.brush_flow;
        color = vec4f(mix(color.rgb, params.brush_color.rgb, amount), 1.0);
    }

    textureStore(dst_tex, vec2i(gid.xy), color);
}
