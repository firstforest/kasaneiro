// rewet.wgsl — Wet the Layer(M2、Rebelle / Painter の Wet Entire Layer 式)。
// 手動ボタンで 1 回だけ走る。キャンバス全面を濡らし(マスク=1)、水を rewet_water
// だけ足す。沈着顔料は transfer.wgsl の脱着(lift_rate × 水量)で自然に再浮遊する
// ので、ここでは顔料に触らない。全面ウォッシュや wet-on-wet の下地作りにも使える。
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

    textureStore(dst_water, ip, vec4f(cell.r + params.rewet_water, cell.gb, 1.0));
    textureStore(dst_susp, ip, textureLoad(src_susp, ip, 0));
    textureStore(dst_dep, ip, textureLoad(src_dep, ip, 0));
}
