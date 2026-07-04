// display.wgsl — キャンバステクスチャを egui のキャンバス領域に表示する。
// M1 以降、H4(デバッグ表示切替: 水量/速度場/顔料…)はこのシェーダーを拡張する。

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // 画面いっぱいの三角形(頂点バッファ不要)
    var out: VsOut;
    let uv = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    out.pos = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    return out;
}

@group(0) @binding(0) var canvas_tex: texture_2d<f32>;
@group(0) @binding(1) var canvas_samp: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    return vec4f(textureSample(canvas_tex, canvas_samp, in.uv).rgb, 1.0);
}
