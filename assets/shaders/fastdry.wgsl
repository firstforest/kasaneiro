// fastdry.wgsl — Fast Dry(M2、Rebelle 式)。手動ボタンで 1 回だけ走る。
// 「水だけ除去」: 水・速度・濡れマスクをゼロにし、浮遊顔料はその場で沈着させる。
// レイヤーへの焼き込みはしない = 描いた絵はまだ湿レイヤー上にあり、
// 再湿潤(rewet.wgsl)やリフティング(M3)で編集し続けられる。
// にじみを今すぐ止めたいが確定はしたくない、というときに使う。
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
    let susp = textureLoad(src_susp, ip, 0);
    let dep = textureLoad(src_dep, ip, 0);

    textureStore(dst_water, ip, vec4f(0.0));
    textureStore(dst_susp, ip, vec4f(0.0));
    textureStore(dst_dep, ip, max(dep + susp, vec4f(0.0)));
}
