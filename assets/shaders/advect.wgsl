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

// 濡れたタップだけのバイリニア補間(水と浮遊顔料を同じタップ・同じ重みで)と、そのタップの最小・最大
// (MacCormack のリミッタ用)。乾いたセルは水も浮遊顔料も 0 なので、混ぜると取り込んだ分だけ水と色が
// 消える(質量が保存されない)。乾いた紙に囲まれた所(塗り跡の切れ込みの先端など)では毎ステップ消え続けて
// 色が抜け、光る点になるので、乾いたタップは重みから外して濡れたタップで割り直す。
// 濡れたタップが無ければ ok = false
struct Bilinear {
    water: vec4f,
    susp: vec4f,
    water_lo: vec4f,
    water_hi: vec4f,
    susp_lo: vec4f,
    susp_hi: vec4f,
    ok: bool,
}

fn bilinear_wet(p: vec2f) -> Bilinear {
    let q = p - 0.5;
    let base = floor(q);
    let f = q - base;
    let i = vec2i(base);
    let offs = array<vec2i, 4>(vec2i(0, 0), vec2i(1, 0), vec2i(0, 1), vec2i(1, 1));
    let wts = array<f32, 4>((1.0 - f.x) * (1.0 - f.y), f.x * (1.0 - f.y), (1.0 - f.x) * f.y, f.x * f.y);
    var b = Bilinear(vec4f(0.0), vec4f(0.0), vec4f(3.4e38), vec4f(-3.4e38), vec4f(3.4e38), vec4f(-3.4e38), false);
    var wsum = 0.0;
    for (var k = 0; k < 4; k++) {
        let j = i + offs[k];
        let w = load_clamped(src_water, j);
        if (!is_wet(w)) {
            continue;
        }
        let c = load_clamped(src_susp, j);
        b.water += wts[k] * w;
        b.susp += wts[k] * c;
        wsum += wts[k];
        b.water_lo = min(b.water_lo, w);
        b.water_hi = max(b.water_hi, w);
        b.susp_lo = min(b.susp_lo, c);
        b.susp_hi = max(b.susp_hi, c);
    }
    if (wsum > 1e-6) {
        b.water /= wsum;
        b.susp /= wsum;
        b.ok = true;
    }
    return b;
}

// 格子点 j の前進移流の値 φ̂_j = bilinear(src, p_j − dt·v_j)。乾いたセルは速度 0 なので自セル値
fn forward_at(j: vec2i) -> array<vec4f, 2> {
    let own = load_clamped(src_water, j);
    let b = bilinear_wet(vec2f(j) + 0.5 - params.dt * own.gb);
    if (!b.ok) {
        return array<vec4f, 2>(own, load_clamped(src_susp, j));
    }
    return array<vec4f, 2>(b.water, b.susp);
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

    // 乾いたセルは素通し(wet-area mask)。濡れたセルのバックトレースは乾いたタップを使わない(bilinear_wet)
    if (!is_wet(cell)) {
        textureStore(dst_water, ip, cell);
        textureStore(dst_susp, ip, textureLoad(src_susp, ip, 0));
        return;
    }

    // 自セルの速度で遡って(backtrace)水量・速度・浮遊顔料を取り直す(φ̂)
    let pos = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let back = pos - params.dt * cell.gb;
    var b = bilinear_wet(back);
    if (!b.ok) {
        // 遡った先が乾いた所だけ: 自セルの値のまま
        let own_s = textureLoad(src_susp, ip, 0);
        b = Bilinear(cell, own_s, cell, cell, own_s, own_s, true);
    }
    var s = b.water;
    var susp = b.susp;

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
        let cw = clamp(b.water + 0.5 * (cell - tw), b.water_lo, b.water_hi);
        let cs = clamp(b.susp + 0.5 * (src_s - ts), b.susp_lo, b.susp_hi);
        let k = clamp(params.advect_sharp, 0.0, 1.0);
        s = mix(b.water, cw, k);
        susp = mix(b.susp, cs, k);
    }

    // 水量・顔料は非負クランプ。a(濡れマスク)は移流させず自セルの値を保持(場に固定のため)
    textureStore(dst_water, ip, vec4f(max(s.r, 0.0), s.gb, cell.a));
    textureStore(dst_susp, ip, max(susp, vec4f(0.0)));
}
