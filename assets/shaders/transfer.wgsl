// transfer.wgsl — 吸着/脱着+蒸発パス(M1b)。Curtis 1997 の TransferPigment の簡略版。
// 浮遊顔料 ⇄ 沈着顔料の交換と、濡れ領域の水の蒸発を 1 パスで行う。
// 浮遊顔料の拡散は diffuse.wgsl(反復パス)に分離した。
// 粒状化(M1d): 紙の凹部(ハイト低)ほど吸着を強め、紙目に顔料が溜まる。
// 乾燥によるレイヤー焼き込みは M2 で足す。
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
    let susp = textureLoad(src_susp, ip, 0);
    let dep = textureLoad(src_dep, ip, 0);

    // 乾いたセルは素通し(wet-area mask。顔料の交換も蒸発も濡れ領域だけ)
    if (!is_wet(cell)) {
        textureStore(dst_water, ip, cell);
        textureStore(dst_susp, ip, susp);
        textureStore(dst_dep, ip, dep);
        return;
    }

    let w = clamp(cell.r, 0.0, 1.0);

    // 吸着(沈着): 水が少ないほど強く働く → 乾きかけの場所で色が定着する。
    // 粒状化(M1d): 凹部(h=0)で ×(1+gran)、凸部(h=1)で ×(1-gran) → 紙目に顔料が溜まる
    let h = textureLoad(paper_tex, ip, 0).r;
    let gran = max(1.0 + params.paper_gran * (1.0 - 2.0 * h), 0.0);
    let down_rate = clamp(params.deposit_rate * params.dt * (2.0 - w) * gran, 0.0, 1.0);
    // 脱着(再浮遊): 水が多い場所ほど沈着顔料が浮き上がる
    let up_rate = clamp(params.lift_rate * params.dt * w, 0.0, 1.0);
    let down = susp * down_rate;
    let up = dep * up_rate;

    // 蒸発: 濡れ領域の水を一定量減らす(マスクは残す。乾燥処理は M2)
    let water = max(cell.r - params.evap_rate * params.dt, 0.0);

    textureStore(dst_water, ip, vec4f(water, cell.gb, cell.a));
    textureStore(dst_susp, ip, max(susp - down + up, vec4f(0.0)));
    textureStore(dst_dep, ip, max(dep + down - up, vec4f(0.0)));
}
