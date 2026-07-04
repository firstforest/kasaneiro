// diffuse.wgsl — 浮遊顔料の拡散パス(M1b)。フィックの法則の陽解法。
// 1 dispatch = 1 反復。CPU 側(gpu/mod.rs)が diffuse_iters 回 ping-pong で呼ぶ
// (relax.wgsl と同じ方式)。陽解法の安定条件で 1 反復の係数は 0.2 までなので、
// 速いにじみは反復回数で稼ぐ: 実効的な拡散速度 = pigment_diffuse × diffuse_iters。
// 水筆で描いた水路に顔料溜まりを接続したとき、色が水路へ広がっていく動きはここが作る。
// 水と沈着顔料はこのパスでは変更しない(素通しで dst へコピー)。
// 先頭に common.wgsl が連結される。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var dst_water: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var dst_susp: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var src_dep: texture_2d<f32>;
@group(0) @binding(5) var dst_dep: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_water);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_water, ip, 0);
    let susp = textureLoad(src_susp, ip, 0);

    // 水と沈着顔料は素通し(ping-pong のため必ず書く)
    textureStore(dst_water, ip, cell);
    textureStore(dst_dep, ip, textureLoad(src_dep, ip, 0));

    // 乾いたセルは拡散に参加しない(wet-area mask)
    if (!is_wet(cell)) {
        textureStore(dst_susp, ip, susp);
        return;
    }

    // 濡れたセル同士だけで交換し(乾いた隣はフラックス 0 = Neumann 境界)、
    // 対になるフラックスが対称なので顔料の総量は保存される。
    // 水が少ない場所へはにじみにくいよう、双方の水量の平均で重み付けする。
    let n_l = load_clamped(src_water, ip + vec2i(-1, 0));
    let n_r = load_clamped(src_water, ip + vec2i(1, 0));
    let n_u = load_clamped(src_water, ip + vec2i(0, -1));
    let n_d = load_clamped(src_water, ip + vec2i(0, 1));
    var flux = vec4f(0.0);
    if (is_wet(n_l)) {
        flux += clamp(0.5 * (cell.r + n_l.r), 0.0, 1.0)
            * (load_clamped(src_susp, ip + vec2i(-1, 0)) - susp);
    }
    if (is_wet(n_r)) {
        flux += clamp(0.5 * (cell.r + n_r.r), 0.0, 1.0)
            * (load_clamped(src_susp, ip + vec2i(1, 0)) - susp);
    }
    if (is_wet(n_u)) {
        flux += clamp(0.5 * (cell.r + n_u.r), 0.0, 1.0)
            * (load_clamped(src_susp, ip + vec2i(0, -1)) - susp);
    }
    if (is_wet(n_d)) {
        flux += clamp(0.5 * (cell.r + n_d.r), 0.0, 1.0)
            * (load_clamped(src_susp, ip + vec2i(0, 1)) - susp);
    }
    // 陽解法の安定条件: 係数は 4 近傍合計で 1 を超えないよう 0.2 に制限
    let k = min(params.pigment_diffuse * params.dt, 0.2);
    textureStore(dst_susp, ip, max(susp + k * flux, vec4f(0.0)));
}
