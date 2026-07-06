// relax.wgsl — 発散の反復緩和パス(M1a)。Curtis 1997 の RelaxDivergence: δ = −ξ·div。
// 1 dispatch = 1 反復。CPU 側(gpu/mod.rs)が relax_iters 回 ping-pong で呼ぶ。
// 元論文はスタガード格子でセルの δ を周囲の速度に散布(scatter)するが、
// GPU では gather 形式に書き換える: 各セルが左右上下の δ を集めて自分の速度を直す。
// 顔料テクスチャはこのパスでは変更しない(素通しで dst へコピー)。
// 先頭に common.wgsl が連結される。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var dst_water: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var dst_susp: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var src_dep: texture_2d<f32>;
@group(0) @binding(5) var dst_dep: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;
// アクティブタイル(M6): タイル有効フラグ。非アクティブなタイルは素通しして計算を省く
@group(0) @binding(11) var<storage, read> tile_active: array<u32>;

// 中心差分による速度の発散
fn divergence(p: vec2i) -> f32 {
    let l = load_clamped(src_water, p + vec2i(-1, 0)).g;
    let r = load_clamped(src_water, p + vec2i(1, 0)).g;
    let u = load_clamped(src_water, p + vec2i(0, -1)).b;
    let d = load_clamped(src_water, p + vec2i(0, 1)).b;
    return 0.5 * ((r - l) + (d - u));
}

fn delta(p: vec2i) -> f32 {
    // 乾いたセルは緩和に参加しない(δ=0)。マスク境界は壁として振る舞う
    if (!is_wet(load_clamped(src_water, p))) {
        return 0.0;
    }
    return -params.xi * divergence(p);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_water);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    // アクティブタイル(M6): 非アクティブなら 3 テクスチャを素通し(ping-pong 一貫性)して return
    if (tile_active[tile_index_of(gid.xy)] == 0u) {
        let cp = vec2i(gid.xy);
        textureStore(dst_water, cp, textureLoad(src_water, cp, 0));
        textureStore(dst_susp, cp, textureLoad(src_susp, cp, 0));
        textureStore(dst_dep, cp, textureLoad(src_dep, cp, 0));
        return;
    }
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_water, ip, 0);

    // 顔料は素通し(ping-pong のため必ず書く)
    textureStore(dst_susp, ip, textureLoad(src_susp, ip, 0));
    textureStore(dst_dep, ip, textureLoad(src_dep, ip, 0));

    // 乾いたセルは素通し(wet-area mask)
    if (!is_wet(cell)) {
        textureStore(dst_water, ip, cell);
        return;
    }

    // 水量: 発散している(流出超過の)セルから水を減らし、収束セルに足す(非負クランプ)
    let water = max(cell.r + delta(ip), 0.0);

    // 速度: 隣接セルの δ を圧力のように受けて発散を打ち消す方向へ補正
    let u = cell.g + 0.5 * (delta(ip + vec2i(-1, 0)) - delta(ip + vec2i(1, 0)));
    let v = cell.b + 0.5 * (delta(ip + vec2i(0, -1)) - delta(ip + vec2i(0, 1)));

    textureStore(dst_water, ip, vec4f(water, u, v, cell.a));
}
