// advect.wgsl — 移流パス(M1a: 水と速度 / M1b: 浮遊顔料)。
// セミラグランジアン法(速度に沿って dt だけ遡った位置の値をバイリニア補間で持ってくる、
// Stam の安定流体法)に MacCormack の誤差補正を advect_sharp の割合で掛ける。
// バイリニア補間は毎ステップ少しずつぼかすので、補正なしでは流れが作った筋が数十ステップで溶ける。
// 浮遊顔料は水と同じ速度場・同じバックトレースで運ぶ。
// 沈着顔料は紙に固定なので移流しない(素通し)。
// 【差し替えポイント】移流スキームを変える(風上差分など)場合はこのファイルだけ書き換える。
// 先頭に common.wgsl が連結される。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var dst_water: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var dst_susp: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var src_dep: texture_2d<f32>;
@group(0) @binding(5) var dst_dep: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;
// アクティブタイル(M6): タイルの状態(TILE_COLD / TILE_COPY / TILE_ACTIVE。tiledilate.wgsl 参照)
@group(0) @binding(11) var<storage, read> tile_active: array<u32>;

// バイリニア補間と、補間に使った4タップの最小・最大(MacCormack のリミッタ用)
struct Bilinear {
    v: vec4f,
    lo: vec4f,
    hi: vec4f,
}

fn bilinear_mm(t: texture_2d<f32>, p: vec2f) -> Bilinear {
    let q = p - 0.5;
    let base = floor(q);
    let f = q - base;
    let i = vec2i(base);
    let c00 = load_clamped(t, i);
    let c10 = load_clamped(t, i + vec2i(1, 0));
    let c01 = load_clamped(t, i + vec2i(0, 1));
    let c11 = load_clamped(t, i + vec2i(1, 1));
    return Bilinear(
        mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y),
        min(min(c00, c10), min(c01, c11)),
        max(max(c00, c10), max(c01, c11)),
    );
}

// 格子点 j の前進移流の値 φ̂_j = bilinear(src, p_j − dt·v_j)。乾いたセルは速度 0 なので自セル値
fn forward_at(j: vec2i) -> array<vec4f, 2> {
    let v = load_clamped(src_water, j).gb;
    let back = vec2f(j) + 0.5 - params.dt * v;
    return array<vec4f, 2>(load_bilinear(src_water, back), load_bilinear(src_susp, back));
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

    // 沈着顔料は移流しない(素通し。ping-pong のため必ず書く)
    textureStore(dst_dep, ip, textureLoad(src_dep, ip, 0));

    // 乾いたセルは素通し(wet-area mask)。splat が水・顔料と同時にマスクを立てるため
    // 乾いたセルの水量・浮遊顔料は常に 0 で、濡れたセルのバックトレースが乾いた領域を
    // サンプルしても水や色は湧かない(薄まるだけ)
    if (!is_wet(cell)) {
        textureStore(dst_water, ip, cell);
        textureStore(dst_susp, ip, textureLoad(src_susp, ip, 0));
        return;
    }

    // 自セルの速度で遡って(backtrace)水量・速度・浮遊顔料を取り直す(φ̂)
    let pos = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let back = pos - params.dt * cell.gb;
    let fw = bilinear_mm(src_water, back);
    let fs = bilinear_mm(src_susp, back);
    var s = fw.v;
    var susp = fs.v;

    // MacCormack: φ̂ を逆向きに dt 進めた φ̃ と元の φ の差が往復の誤差なので、その半分を戻す。
    // φ̂ の格子値は x + dt·v を囲む4格子点でその場で前進移流して求める(1パスで完結させるため)。
    // 補正後の値を前進サンプルの4タップの範囲に収めて(リミッタ)振動と負値を防ぐ
    if (params.advect_sharp > 0.0) {
        let q = pos + params.dt * cell.gb - 0.5;
        let base = floor(q);
        let f = q - base;
        let i = vec2i(base);
        let a00 = forward_at(i);
        let a10 = forward_at(i + vec2i(1, 0));
        let a01 = forward_at(i + vec2i(0, 1));
        let a11 = forward_at(i + vec2i(1, 1));
        let tw = mix(mix(a00[0], a10[0], f.x), mix(a01[0], a11[0], f.x), f.y);
        let ts = mix(mix(a00[1], a10[1], f.x), mix(a01[1], a11[1], f.x), f.y);
        let src_s = textureLoad(src_susp, ip, 0);
        let cw = clamp(fw.v + 0.5 * (cell - tw), fw.lo, fw.hi);
        let cs = clamp(fs.v + 0.5 * (src_s - ts), fs.lo, fs.hi);
        let k = clamp(params.advect_sharp, 0.0, 1.0);
        s = mix(fw.v, cw, k);
        susp = mix(fs.v, cs, k);
    }

    // 水量・顔料は非負クランプ。a(濡れマスク)は移流させず自セルの値を保持(場に固定のため)
    textureStore(dst_water, ip, vec4f(max(s.r, 0.0), s.gb, cell.a));
    textureStore(dst_susp, ip, max(susp, vec4f(0.0)));
}
