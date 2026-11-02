// diffuse.wgsl — 浮遊顔料+水の拡散パス(M1b)。フィックの法則の陽解法。
// 1 dispatch = 1 反復。CPU 側(gpu/callback.rs の prepare)が diffuse_iters 回 ping-pong で呼ぶ
// (relax.wgsl と同じ方式)。陽解法の安定条件で 1 反復の係数は 0.2 までなので、
// 速いにじみは反復回数で稼ぐ: 実効的な拡散速度 = pigment_diffuse × diffuse_iters。
// 水筆で描いた水路に顔料溜まりを接続したとき、色が水路へ広がっていく動きはここが作る。
// 水の拡散(毛細管、note/05)は「筆で置いた水が濡れた紙を伝ってひとりでに広がる」を作る。
// 沈着顔料はこのパスでは変更しない(素通しで dst へコピー)。
// 先頭に common.wgsl が連結される。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var dst_water: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var dst_susp: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var src_dep: texture_2d<f32>;
@group(0) @binding(5) var dst_dep: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;
// 紙(paper.rs): r = 紙ハイト(paper_drift)/ g, b = 混ざりムラの場2枚(mix_mottle)
@group(0) @binding(8) var paper_tex: texture_2d<f32>;
// 顔料個性: [i] = (密度 ρ, ステイニング ω, 粒状感 γ, 粒の細かさ μ)。ここでは γ と μ を読む
@group(0) @binding(9) var<uniform> pigment: array<vec4f, 4>;
// 清書ペンの線画(M4.5b): 隣接流束の透水率境界。ペン線を挟む2セル間は顔料が拡散しにくい
@group(0) @binding(10) var pen_line_tex: texture_2d<f32>;
// アクティブタイル(M6): タイルの状態(TILE_COLD / TILE_COPY / TILE_ACTIVE。tiledilate.wgsl 参照)
@group(0) @binding(11) var<storage, read> tile_active: array<u32>;

// ペン線を挟んだ透水率(M4.5b): 両隣のどちらかにペン線があれば流束を絞る
fn edge_perm(here: f32, at: vec2i) -> f32 {
    let there = textureLoad(pen_line_tex, clamp(at, vec2i(0), vec2i(textureDimensions(pen_line_tex)) - 1), 0).r;
    return clamp(1.0 - params.line_block * max(here, there), 0.0, 1.0);
}

// 隣接2セル間の流束の水依存重み: (両セルの水量平均)^γ。
// γ>1 で「水がたっぷりのときは傾斜がなくても自由に混ざり、
// 水が引いてくると急に混ざらなくなる」カーブになる(乾きかけの縁は形が残る)。
// γ=4 は調整済みの定数(docs/note/05 §2: w=1.0 で全開・w=0.7 で約1/4・w=0.5 で約1/16)。
// 再調整はホットリロード(H1)でここを直接編集
const DIFFUSE_GAMMA: f32 = 4.0;

fn wet_weight(a: f32, b: f32) -> f32 {
    return pow(clamp(0.5 * (a + b), 0.0, 1.0), DIFFUSE_GAMMA);
}

// 顔料の溜まりやすさのポテンシャル φ(顔料ごと、小さいほど溜まる)による、隣接2セル間の流束のドリフト。
// 隣から自セルへの流束 = 重み × (c_隣 × (1+d) − c_自 × (1−d))、d = ½ × (φ_隣 − φ_自)。
// d は隣から見ると符号が反転するので流束は対称で顔料の総量は保存され、d=0 でただの拡散に戻る。
// 流束 0 になるのは c ∝ (1−d)/(1+d) ≈ exp(−φ) のときで、濡れている間はこの分布へ向かう。
// φ は2つの規模の和(potential):
// - 紙の谷への寄り: paper_drift × γ × 紙ハイト。紙目の細かさで、粒状感 γ の高い顔料だけが谷に溜まる
// - 混ざりムラ: mix_mottle × 斑の場。数十 px 規模で全顔料に効き、顔料ごとに場の組み合わせを
//   変えるので混ぜた色の比率が揺れる(同じ場だと濃淡だけ揃って揺れ、色は揺れない)
// d を ±DRIFT_MAX に収めるのは陽解法の安定条件のため: 自セルの係数の合計は
// 4 近傍 × 0.2 × (1 + DRIFT_MAX) ≦ 1、隣の係数 (1+d) は正のまま
const DRIFT_MAX: f32 = 0.25;

// 混ざりムラの場を顔料ごとに作る: 2枚の場 (g, b) を 0°/90°/45°/135° の向きで組み合わせる。
// どの2顔料も組み合わせが違うので、どの混色でも比率が揺れる
const MOTTLE_MIX: mat4x2f = mat4x2f(
    vec2f(1.0, 0.0),
    vec2f(0.0, 1.0),
    vec2f(0.70710677, 0.70710677),
    vec2f(0.70710677, -0.70710677),
);

fn potential(at: vec2i, gran: vec4f) -> vec4f {
    let p = load_clamped(paper_tex, at);
    let m = p.gb * MOTTLE_MIX;  // 成分 i = dot(場, MOTTLE_MIX[i])
    return params.paper_drift * gran * p.r + params.mix_mottle * m;
}

fn pigment_flux(w: f32, c_n: vec4f, c: vec4f, dphi: vec4f) -> vec4f {
    let d = clamp(0.5 * dphi, vec4f(-DRIFT_MAX), vec4f(DRIFT_MAX));
    return w * (c_n * (1.0 + d) - c * (1.0 - d));
}

// 水の毛細管拡散(note/05): 濡れマスク内で水量そのものを拡散させる(簡易 Richards)。
// 「たっぷりの水を置けば濡れた紙を伝ってひとりでに広がり、水位が下がるにつれ止まる」を
// 一次原理で作る。重みは両セルの max^γ: 平均ではなく max なのは、濡れ側の水が乾きかけ側へ
// 「吸い込まれる」毛細管の向き(受け側が乾いているほど吸引は強い)を殺さないため。
// 水量が全体に低くなると max も下がって流束が消える=水が引くと自然に止まる。
// 顔料は wet_weight(平均^4)で後追いするので、水が先行して顔料がにじみ寄る
// (クロマトグラフィー的な水の輪も出る)。係数は per-iter 0.2 が陽解法の安定上限
const WATER_DIFFUSE: f32 = 0.15;  // 実効速度 = min(この値×dt, 0.2) × diffuse_iters
const WATER_GAMMA: f32 = 2.0;     // 水量→毛細管の効きのカーブ(pigment の γ=4 より緩め)

fn cap_weight(a: f32, b: f32) -> f32 {
    return pow(clamp(max(a, b), 0.0, 1.0), WATER_GAMMA);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_water);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    // アクティブタイル(M6): 計算しないタイルは、2枚が食い違いうる(COPY)ときだけ 3 テクスチャを
    // 素通しでコピーして揃え、一致している(COLD)ときは何も書かずに return
    let tile = tile_active[tile_index_of(gid.xy)];
    if (tile != TILE_ACTIVE) {
        if (tile == TILE_COPY) {
            let cp = vec2i(gid.xy);
            textureStore(dst_water, cp, textureLoad(src_water, cp, 0));
            textureStore(dst_susp, cp, textureLoad(src_susp, cp, 0));
            textureStore(dst_dep, cp, textureLoad(src_dep, cp, 0));
        }
        return;
    }
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_water, ip, 0);
    let susp = textureLoad(src_susp, ip, 0);

    // 沈着顔料は素通し(ping-pong のため必ず書く)
    textureStore(dst_dep, ip, textureLoad(src_dep, ip, 0));

    // 乾いたセルは拡散に参加しない(wet-area mask)
    if (!is_wet(cell)) {
        textureStore(dst_water, ip, cell);
        textureStore(dst_susp, ip, susp);
        return;
    }

    // 濡れたセル同士だけで交換し(乾いた隣はフラックス 0 = Neumann 境界)、
    // 対になるフラックスが対称なので顔料の総量は保存される。
    // 重みは双方の水量平均^γ(wet_weight): 水がたっぷりなら傾斜がなくても濃度差だけで
    // 自由に混ざり、水が引いてくると急に混ざらなくなる(DIFFUSE_GAMMA)
    let n_l = load_clamped(src_water, ip + vec2i(-1, 0));
    let n_r = load_clamped(src_water, ip + vec2i(1, 0));
    let n_u = load_clamped(src_water, ip + vec2i(0, -1));
    let n_d = load_clamped(src_water, ip + vec2i(0, 1));
    // 透水率(M4.5b): 自セルのペン濃度と各隣接の max で流束を絞る(線を挟むと拡散しない)
    let pen_c = textureLoad(pen_line_tex, ip, 0).r;
    let gran = vec4f(pigment[0].z, pigment[1].z, pigment[2].z, pigment[3].z);
    let phi_c = potential(ip, gran);
    var flux = vec4f(0.0);
    var wflux = 0.0;
    if (is_wet(n_l)) {
        let perm = edge_perm(pen_c, ip + vec2i(-1, 0));
        flux += pigment_flux(wet_weight(cell.r, n_l.r) * perm,
            load_clamped(src_susp, ip + vec2i(-1, 0)), susp,
            potential(ip + vec2i(-1, 0), gran) - phi_c);
        wflux += cap_weight(cell.r, n_l.r) * perm * (n_l.r - cell.r);
    }
    if (is_wet(n_r)) {
        let perm = edge_perm(pen_c, ip + vec2i(1, 0));
        flux += pigment_flux(wet_weight(cell.r, n_r.r) * perm,
            load_clamped(src_susp, ip + vec2i(1, 0)), susp,
            potential(ip + vec2i(1, 0), gran) - phi_c);
        wflux += cap_weight(cell.r, n_r.r) * perm * (n_r.r - cell.r);
    }
    if (is_wet(n_u)) {
        let perm = edge_perm(pen_c, ip + vec2i(0, -1));
        flux += pigment_flux(wet_weight(cell.r, n_u.r) * perm,
            load_clamped(src_susp, ip + vec2i(0, -1)), susp,
            potential(ip + vec2i(0, -1), gran) - phi_c);
        wflux += cap_weight(cell.r, n_u.r) * perm * (n_u.r - cell.r);
    }
    if (is_wet(n_d)) {
        let perm = edge_perm(pen_c, ip + vec2i(0, 1));
        flux += pigment_flux(wet_weight(cell.r, n_d.r) * perm,
            load_clamped(src_susp, ip + vec2i(0, 1)), susp,
            potential(ip + vec2i(0, 1), gran) - phi_c);
        wflux += cap_weight(cell.r, n_d.r) * perm * (n_d.r - cell.r);
    }
    // 粒の細かさ μ(分離色): 顔料ごとの拡散倍率。細かい顔料(μ>1)は水に乗って遠くまで
    // 運ばれ、粗い粒(μ<1)はその場に残る。2顔料を混ぜて置いたとき μ の差で縁と中心に
    // 色が分かれる=分離色の主役。μ は per-channel の定数なので隣接ペアの流束は対称のまま
    // (質量保存は崩れない)。
    // 陽解法の安定条件: 係数は 4 近傍合計で 1 を超えないよう、チャンネルごとに 0.2 に制限
    let mob = vec4f(pigment[0].w, pigment[1].w, pigment[2].w, pigment[3].w);
    // μ は CPU 側(physics_uniform)で 0 以上に収めてあるが、係数は負にならないよう下も切る
    let k = clamp(vec4f(params.pigment_diffuse * params.dt) * mob, vec4f(0.0), vec4f(0.2));
    textureStore(dst_susp, ip, max(susp + k * flux, vec4f(0.0)));
    // 水の毛細管拡散: 対称な流束なので水の総量は保存される(速度・マスクは変えない)
    let kw = min(WATER_DIFFUSE * params.dt, 0.2);
    textureStore(dst_water, ip, vec4f(max(cell.r + kw * wflux, 0.0), cell.gb, cell.a));
}
