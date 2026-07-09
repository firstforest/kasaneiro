// transfer.wgsl — 吸着/脱着+蒸発パス(M1b)。Curtis 1997 の TransferPigment の簡略版。
// 浮遊顔料 ⇄ 沈着顔料の交換と、濡れ領域の水の蒸発を 1 パスで行う。
// 浮遊顔料の拡散は diffuse.wgsl(反復パス)に分離した。
// 粒状化(M1d): 紙の凹部(ハイト低)ほど吸着を強め、紙目に顔料が溜まる。
// 顔料個性(M3): 吸着/脱着率を顔料ごとの密度 ρ・ステイニング ω・粒状感 γ で変調する
// (binding 9)。ステイニング顔料は脱着しにくく、粒状顔料ほど紙目に溜まる。
// 乾燥によるレイヤー焼き込みは M2、リフト/消去ツールは splat.wgsl。
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
// 顔料個性(M3): [i] = (密度 ρ, ステイニング ω, 粒状感 γ, 予備)
@group(0) @binding(9) var<uniform> pigment: array<vec4f, 4>;
// アクティブタイル(M6): タイル有効フラグ。非アクティブなタイルは素通しして計算を省く
@group(0) @binding(11) var<storage, read> tile_active: array<u32>;

// 吸着の水依存カーブ(note/07 で1本化): 乾きかけ(w→0)で 2.0、たっぷり(w→1)で
// DEPOSIT_WET_FLOOR まで単調減少。たっぷりの水の中では顔料が沈まず浮遊し続けて自由に
// 流れ・混ざり、沈着は水が引いてから始まる(旧「水持ち」)。
// 旧実装の (2−w)×(1−0.7w) の2重ヒューリスティックを単一カーブに置き換えたもので、
// mix(2.0, 0.3, w^0.7) は旧カーブとほぼ一致する(w=0.5 で 0.95 vs 旧 0.975)。
// 再調整はホットリロード(H1)でここを直接編集
const DEPOSIT_WET_FLOOR: f32 = 0.3; // w=1 での吸着倍率(0 にすると水中で全く沈まない)
const DEPOSIT_GAMMA: f32 = 0.7;     // カーブの立ち上がり(小さいほど早めに吸着が戻る)

fn deposit_weight(w: f32) -> f32 {
    return mix(2.0, DEPOSIT_WET_FLOOR, pow(w, DEPOSIT_GAMMA));
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
    let h = textureLoad(paper_tex, ip, 0).r;

    // 顔料ごとに吸着/脱着率が変わる(M3: 顔料個性)。deposit/lift は 4 チャンネルまとめて vec4 で。
    var down = vec4f(0.0);
    var up = vec4f(0.0);
    for (var c = 0u; c < 4u; c++) {
        let rho = pigment[c].x;    // 密度: 重い顔料ほど早く沈着
        let omega = pigment[c].y;  // ステイニング: 高いほど剥がれない(脱着を (1−ω) で抑制)
        let gamma = pigment[c].z;  // 粒状感: 紙目への反応(paper_gran を顔料ごとにスケール)
        // 吸着(沈着): 水が少ないほど強い → 乾きかけで定着(deposit_weight の単一カーブ)。
        // 粒状化(M1d/M3): 凹部(h=0)で強め・凸部(h=1)で弱め、効き幅は顔料の γ に比例
        let gran = max(1.0 + params.paper_gran * gamma * (1.0 - 2.0 * h), 0.0);
        let down_rate = clamp(params.deposit_rate * rho * params.dt * deposit_weight(w) * gran, 0.0, 1.0);
        // 脱着(再浮遊): 水が多い場所ほど浮き上がるが、ステイニング顔料は (1−ω) で残る
        let up_rate = clamp(params.lift_rate * params.dt * w * (1.0 - omega), 0.0, 1.0);
        down[c] = susp[c] * down_rate;
        up[c] = dep[c] * up_rate;
    }

    // 蒸発: 濡れ領域の水を一定量減らす(マスクは残す。乾燥処理は M2)
    let water = max(cell.r - params.evap_rate * params.dt, 0.0);

    textureStore(dst_water, ip, vec4f(water, cell.gb, cell.a));
    textureStore(dst_susp, ip, max(susp - down + up, vec4f(0.0)));
    textureStore(dst_dep, ip, max(dep + down - up, vec4f(0.0)));
}
