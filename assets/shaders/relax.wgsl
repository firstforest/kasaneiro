// relax.wgsl — 発散の反復緩和パス(M1a)。Curtis 1997 の RelaxDivergence: δ = −ξ·div。
// 1 dispatch = 1 反復。CPU 側(gpu/mod.rs)が relax_iters 回 ping-pong で呼ぶ。
// 元論文はスタガード格子でセルの δ を周囲の速度に散布(scatter)するが、
// GPU では gather 形式に書き換える: 各セルが左右上下の δ を集めて自分の速度を直す。
// 先頭に common.wgsl が連結される。

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> params: SimParams;
@group(0) @binding(3) var<storage, read> splat_buf: SplatBuffer;

// 中心差分による速度の発散
fn divergence(p: vec2i) -> f32 {
    let l = load_clamped(src_tex, p + vec2i(-1, 0)).g;
    let r = load_clamped(src_tex, p + vec2i(1, 0)).g;
    let u = load_clamped(src_tex, p + vec2i(0, -1)).b;
    let d = load_clamped(src_tex, p + vec2i(0, 1)).b;
    return 0.5 * ((r - l) + (d - u));
}

fn delta(p: vec2i) -> f32 {
    return -params.xi * divergence(p);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_tex);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_tex, ip, 0);

    // 水量: 発散している(流出超過の)セルから水を減らし、収束セルに足す(非負クランプ)
    let water = max(cell.r + delta(ip), 0.0);

    // 速度: 隣接セルの δ を圧力のように受けて発散を打ち消す方向へ補正
    let u = cell.g + 0.5 * (delta(ip + vec2i(-1, 0)) - delta(ip + vec2i(1, 0)));
    let v = cell.b + 0.5 * (delta(ip + vec2i(0, -1)) - delta(ip + vec2i(0, 1)));

    textureStore(dst_tex, ip, vec4f(water, u, v, cell.a));
}
