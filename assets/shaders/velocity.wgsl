// velocity.wgsl — 速度更新パス(M1a)。
// 浅水方程式の簡略版: 水面(=水深)の勾配で加速し、減衰をかけ、CFL 的に上限クランプする。
// 先頭に common.wgsl が連結される。

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
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_tex, ip, 0);

    // 水深の中心差分勾配(M1d で紙ハイト h を足して ∇(w+h) にする)
    let w_l = load_clamped(src_tex, ip + vec2i(-1, 0)).r;
    let w_r = load_clamped(src_tex, ip + vec2i(1, 0)).r;
    let w_u = load_clamped(src_tex, ip + vec2i(0, -1)).r;
    let w_d = load_clamped(src_tex, ip + vec2i(0, 1)).r;
    let grad = vec2f(w_r - w_l, w_d - w_u) * 0.5;

    // 勾配で加速(水は低い方へ)+ 減衰(粘性の代用)
    var vel = (cell.gb - params.accel * params.dt * grad) * (1.0 - params.damping);

    // CFL 的制約: 1 ステップで vel_max セル以上動かさない
    let speed = length(vel);
    if (speed > params.vel_max) {
        vel *= params.vel_max / speed;
    }

    // 境界セルは速度ゼロ(水がキャンバス外へ逃げない・端に張り付かない)
    if (gid.x == 0u || gid.y == 0u || gid.x == dims.x - 1u || gid.y == dims.y - 1u) {
        vel = vec2f(0.0);
    }

    textureStore(dst_tex, ip, vec4f(cell.r, vel, cell.a));
}
