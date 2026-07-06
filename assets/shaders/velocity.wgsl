// velocity.wgsl — 速度更新パス(M1a)。
// 浅水方程式の簡略版: 水面(=水深+紙ハイト)の勾配で加速し、減衰をかけ、CFL 的に上限クランプする。
// 紙ハイト(M1d)はここで2箇所に効く:
//   ①水面 = 水深 + paper_amp × 高さ → 水が紙の谷へ流れ、紙目に沿ったストリークが出る
//   ②にじみ拡張(wet_expand)を紙目で変調 → 濡れ前線が谷を選んで進み、縁が不規則になる
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
@group(0) @binding(8) var paper_tex: texture_2d<f32>;
// 清書ペンの線画(M4.5b): 透水率 perm = 1 − line_block×ペン濃度 の境界。速度場とにじみ拡張に効く
@group(0) @binding(10) var pen_line_tex: texture_2d<f32>;
// アクティブタイル(M6): タイル有効フラグ。非アクティブなタイルは素通しして計算を省く
@group(0) @binding(11) var<storage, read> tile_active: array<u32>;

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
    // 透水率(M4.5b): ペン線が濃いほど 0 に近づき、水の動きを止める
    let perm = clamp(1.0 - params.line_block * textureLoad(pen_line_tex, ip, 0).r, 0.0, 1.0);

    // 顔料は素通し(ping-pong のため必ず書く)
    textureStore(dst_susp, ip, textureLoad(src_susp, ip, 0));
    textureStore(dst_dep, ip, textureLoad(src_dep, ip, 0));

    // 乾いたセルは水が動かない(wet-area mask)。速度ゼロで素通し。
    // にじみ拡張(wet_expand > 0): 濡れた隣の水量に比例してマスク値を蓄積し、
    // is_wet の閾値 0.5 を超えたら「濡れ」に昇格する(水が多い縁ほど速く外へ育つ)
    if (!is_wet(cell)) {
        var seep = 0.0;
        let n_l = load_clamped(src_water, ip + vec2i(-1, 0));
        let n_r = load_clamped(src_water, ip + vec2i(1, 0));
        let n_u = load_clamped(src_water, ip + vec2i(0, -1));
        let n_d = load_clamped(src_water, ip + vec2i(0, 1));
        seep += select(0.0, n_l.r, is_wet(n_l));
        seep += select(0.0, n_r.r, is_wet(n_r));
        seep += select(0.0, n_u.r, is_wet(n_u));
        seep += select(0.0, n_d.r, is_wet(n_d));
        // 紙目変調(M1d): 谷(h=0)は最大2倍、山(h=1)はほぼ 0 倍で前線が進む
        let h = textureLoad(paper_tex, ip, 0).r;
        let seep_scale = mix(1.0, 2.0 * (1.0 - h), params.paper_wet);
        // 透水率(M4.5b): ペン線の乾いたセルへは濡れ前線が染み込みにくい
        let mask = min(cell.a + params.wet_expand * params.dt * seep * seep_scale * perm, 1.0);
        textureStore(dst_water, ip, vec4f(cell.r, 0.0, 0.0, mask));
        return;
    }

    // 水面(= 水深 + 紙ハイト)の中心差分勾配。水は紙の谷へも流れる(M1d)。
    // 乾いた隣接セルの水面は自セル値で代用(Neumann 境界)し、マスク境界へ向かう加速を消す
    let s_c = cell.r + params.paper_amp * textureLoad(paper_tex, ip, 0).r;
    let l = load_clamped(src_water, ip + vec2i(-1, 0));
    let r = load_clamped(src_water, ip + vec2i(1, 0));
    let u = load_clamped(src_water, ip + vec2i(0, -1));
    let d = load_clamped(src_water, ip + vec2i(0, 1));
    let h_l = load_clamped(paper_tex, ip + vec2i(-1, 0)).r;
    let h_r = load_clamped(paper_tex, ip + vec2i(1, 0)).r;
    let h_u = load_clamped(paper_tex, ip + vec2i(0, -1)).r;
    let h_d = load_clamped(paper_tex, ip + vec2i(0, 1)).r;
    let w_l = select(s_c, l.r + params.paper_amp * h_l, is_wet(l));
    let w_r = select(s_c, r.r + params.paper_amp * h_r, is_wet(r));
    let w_u = select(s_c, u.r + params.paper_amp * h_u, is_wet(u));
    let w_d = select(s_c, d.r + params.paper_amp * h_d, is_wet(d));
    let grad = vec2f(w_r - w_l, w_d - w_u) * 0.5;

    // 勾配で加速(水は低い方へ)+ 減衰(粘性の代用)
    var vel = (cell.gb - params.accel * params.dt * grad) * (1.0 - params.damping);
    // 透水率(M4.5b): ペン線のセルは速度を殺す = 水がその線を越えて流れない
    vel *= perm;

    // CFL 的制約: 1 ステップで vel_max セル以上動かさない
    let speed = length(vel);
    if (speed > params.vel_max) {
        vel *= params.vel_max / speed;
    }

    // 境界セルは速度ゼロ(水がキャンバス外へ逃げない・端に張り付かない)
    if (gid.x == 0u || gid.y == 0u || gid.x == dims.x - 1u || gid.y == dims.y - 1u) {
        vel = vec2f(0.0);
    }

    textureStore(dst_water, ip, vec4f(cell.r, vel, cell.a));
}
