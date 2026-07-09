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
// 清書ペンの線画(M4.5b): 隣接流束の透水率境界。ペン線を挟む2セル間は顔料が拡散しにくい
@group(0) @binding(10) var pen_line_tex: texture_2d<f32>;
// アクティブタイル(M6): タイル有効フラグ。非アクティブなタイルは素通しして計算を省く
@group(0) @binding(11) var<storage, read> tile_active: array<u32>;

// ペン線を挟んだ透水率(M4.5b): 両隣のどちらかにペン線があれば流束を絞る
fn edge_perm(here: f32, at: vec2i) -> f32 {
    let there = textureLoad(pen_line_tex, clamp(at, vec2i(0), vec2i(textureDimensions(pen_line_tex)) - 1), 0).r;
    return clamp(1.0 - params.line_block * max(here, there), 0.0, 1.0);
}

// 隣接2セル間の流束の水依存重み: (両セルの水量平均)^γ。
// γ=1 は従来の線形。γ>1 で「水がたっぷりのときは傾斜がなくても自由に混ざり、
// 水が引いてくると急に混ざらなくなる」カーブになる(乾きかけの縁は形が残る)。
// γ=4 は調整済みの定数(docs/note/06 §5: w=1.0 で全開・w=0.7 で約1/4・w=0.5 で約1/16)。
// 再調整はホットリロード(H1)でここを直接編集
const DIFFUSE_GAMMA: f32 = 4.0;

fn wet_weight(a: f32, b: f32) -> f32 {
    return pow(clamp(0.5 * (a + b), 0.0, 1.0), DIFFUSE_GAMMA);
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
    // 重みは双方の水量平均^γ(wet_weight): 水がたっぷりなら傾斜がなくても濃度差だけで
    // 自由に混ざり、水が引いてくると急に混ざらなくなる(DIFFUSE_GAMMA)
    let n_l = load_clamped(src_water, ip + vec2i(-1, 0));
    let n_r = load_clamped(src_water, ip + vec2i(1, 0));
    let n_u = load_clamped(src_water, ip + vec2i(0, -1));
    let n_d = load_clamped(src_water, ip + vec2i(0, 1));
    // 透水率(M4.5b): 自セルのペン濃度と各隣接の max で流束を絞る(線を挟むと拡散しない)
    let pen_c = textureLoad(pen_line_tex, ip, 0).r;
    var flux = vec4f(0.0);
    if (is_wet(n_l)) {
        flux += wet_weight(cell.r, n_l.r) * edge_perm(pen_c, ip + vec2i(-1, 0))
            * (load_clamped(src_susp, ip + vec2i(-1, 0)) - susp);
    }
    if (is_wet(n_r)) {
        flux += wet_weight(cell.r, n_r.r) * edge_perm(pen_c, ip + vec2i(1, 0))
            * (load_clamped(src_susp, ip + vec2i(1, 0)) - susp);
    }
    if (is_wet(n_u)) {
        flux += wet_weight(cell.r, n_u.r) * edge_perm(pen_c, ip + vec2i(0, -1))
            * (load_clamped(src_susp, ip + vec2i(0, -1)) - susp);
    }
    if (is_wet(n_d)) {
        flux += wet_weight(cell.r, n_d.r) * edge_perm(pen_c, ip + vec2i(0, 1))
            * (load_clamped(src_susp, ip + vec2i(0, 1)) - susp);
    }
    // 陽解法の安定条件: 係数は 4 近傍合計で 1 を超えないよう 0.2 に制限
    let k = min(params.pigment_diffuse * params.dt, 0.2);
    textureStore(dst_susp, ip, max(susp + k * flux, vec4f(0.0)));
}
