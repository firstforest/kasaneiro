// display.wgsl — シミュレーションテクスチャを egui のキャンバス領域に表示する。
// H4(デバッグ表示切替): params.display_mode で分岐する。
//   0 = 通常(顔料を mixbox 混色で紙の上にレンダリング。濡れている場所はわずかに暗く)
//   1 = 水量ヒートマップ
//   2 = 速度場(色相=方向、明度=大きさ)
//   3 = 湿りオーバーレイ(Rebelle の Show Wet 相当: 通常表示に濡れ領域を青重ね)
//   4 = 浮遊顔料ヒートマップ(4チャンネル合計)
//   5 = 沈着顔料ヒートマップ(4チャンネル合計)
//   6 = 紙ハイト(グレースケール。白=山 / 黒=谷)
//   7 = アクティブタイル(M6。計算中のタイル=緑 / コピーだけのタイル=青 / 何もしないタイル=暗く + タイル格子)
// 先頭に common.wgsl が連結される。
//
// 通常表示の混色(M1c): 4顔料の濃度(浮遊+沈着)と紙を mixbox の latent 空間で
// 線形混合し、latent → RGB 多項式で発色する。latent は CPU 側(crates/pigment/src/lib.rs)が
// mixbox LUT から顔料ごとに1回だけ計算して uniform で渡すため、GPU に LUT は不要。
//
// 乾燥レイヤー(M2): texture array の各スライス = 1 乾燥レイヤー(rgba = 4顔料濃度)。
// 合成は params.compose_mode で切替(H5 の再生で A/B 比較):
//   0 = multiply(M2): 最終色 = 湿レイヤーの紙上発色 × Π(可視な乾燥レイヤーを白地に置いた透過率)
//   1 = KM(M3): 各層を白地/黒地に置いた発色 R_w,R_b から反射率 R=R_b・透過率² T²=(R_w−R_b)(1−R_b)
//       を導き(crates/km/src/lib.rs の簡約)、紙を最下層に下から光学合成する。上に薄い層を重ねるほど下が透ける
//       「内側から光る」グレーズ挙動になる。計算はリニア色空間で行い最後に sRGB へ戻す(note 03 §3)。
// どちらも層「内」の混色は mixbox の latent 混合(render_conc)を通す(黄+青が濁らない M1c の芯)。
//
// 乾燥レイヤーの合成は湿レイヤーと無関係で、乾かす・レイヤーの並びや可視性・濃度倍率を変える・
// compose_mode / pigment_density を変えるときにしか変わらない。そこで compute の入口
// dried_cache_main がキャンバス解像度の合成結果(multiply = 透過率の積 / KM = 紙+乾燥層のリニア反射率)を
// dried_cache へ焼き、fs_main はそれを読むだけにする(作り直す時機は gpu/callback.rs の encode_frame)。

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // 画面いっぱいの三角形(頂点バッファ不要)
    var out: VsOut;
    let uv = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    out.pos = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    return out;
}

@group(0) @binding(0) var water_tex: texture_2d<f32>;
@group(0) @binding(1) var susp_tex: texture_2d<f32>;
@group(0) @binding(2) var dep_tex: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: SimParams;
// mixbox latent(crates/pigment の global_latents / Palette::pigment_latents と同レイアウト。M5c):
//   グローバル光学(先頭 GLOBAL_LATENTS=6): [0],[1]=紙色 / [2],[3]=白(R_w)/ [4],[5]=黒(R_b, KM)
//   パレット枠(pal ごとに PIGMENT_LATENTS=8): 顔料 i は [PAL_BASE(pal)+2i]=(c0..c3), [+2i+1]=RGB 残差
//   pal = 0..MAX_LAYERS-1 は乾燥レイヤーのスロット別(乾かした瞬間に記録=色が遡って変わらない)、
//   pal = LIVE_PALETTE(=MAX_LAYERS) は現行(湿レイヤー)のパレット。総数 LATENT_TOTAL。
//   定数(GLOBAL_LATENTS / PIGMENT_LATENTS / LIVE_PALETTE / LATENT_TOTAL / MAX_LAYERS)は
//   Rust 側(src/gpu/prelude.rs)が生成して先頭に連結する
@group(0) @binding(4) var<uniform> latents: array<vec4f, LATENT_TOTAL>;
// パレット pal の顔料ブロック先頭(latents 内の vec4 インデックス)
fn pal_base(pal: u32) -> u32 { return GLOBAL_LATENTS + pal * PIGMENT_LATENTS; }
@group(0) @binding(5) var paper_tex: texture_2d<f32>;
// 乾燥レイヤー(M2): スライス = レイヤースロット、rgba = 4顔料濃度(dried_cache_main だけが読む)
@group(0) @binding(6) var dried_tex: texture_2d_array<f32>;
// src/gpu/mod.rs の LayerUniform と同レイアウト。
// order[k] = 下から k 番目のレイヤーのスロット番号 / visible_mask の bit k = 下から k 番目の可視性
// (multiply は可換なので順序は見た目に効かず、KM 合成でだけ効く)。
// density[k] = 下から k 番目のレイヤーの濃度倍率(顔料濃度に掛ける。1 = 乾かしたときのまま)。
// has_base = 1 なら合成を紙ではなく下地(base_tex)から始める。
// force_mode = 0 は params.compose_mode に従い、1 / 2 は乗算 / KM で焼く(下地の統合が両方式を焼くため)
struct LayerUniform {
    count: u32,
    visible_mask: u32,
    has_base: u32,
    force_mode: u32,
    order: array<vec4u, LAYER_ORDER_VEC4S>,
    density: array<vec4f, LAYER_ORDER_VEC4S>,
};
@group(0) @binding(7) var<uniform> layers: LayerUniform;
// 下地(rgba16float の2スライス): 下の乾燥レイヤーを焼き固めた合成結果。
// スライス 0 = 乗算の透過率の積 / 1 = 紙+焼いた層の KM 反射率(リニア)。dried_cache_main だけが読む
@group(0) @binding(15) var base_tex: texture_2d_array<f32>;
// 乾燥レイヤーの合成キャッシュ(rgba16float、キャンバス解像度。rgb のみ使う)。
// compose_mode=0 は可視な乾燥レイヤーの multiply 係数、1 は紙+乾燥層の KM 反射率(リニア)。
// fs_main は 13 で読み、dried_cache_main は 14 へ書く(別の bind group)
@group(0) @binding(13) var dried_cache: texture_2d<f32>;
@group(0) @binding(14) var dried_cache_out: texture_storage_2d<rgba16float, write>;
// 線画(M4.5a): r32float ×2。r = インク濃度 0..1(0=線なし)。色より上に合成する
@group(0) @binding(8) var pencil_tex: texture_2d<f32>;
@group(0) @binding(9) var pen_tex: texture_2d<f32>;
// ハイライト(M4.5c): r32float 1枚。r = 不透明度 0..1。合成の最後に白として重ねる(最上段)
@group(0) @binding(10) var highlight_tex: texture_2d<f32>;
// ビューポート変換(M6): 画面 uv → キャンバス uv。
//   canvas_uv = center + R(θ)·(uv − 0.5)·span。
//   center = 画面中心に来るキャンバス uv、span = 1/zoom(1.0=全体、<1.0=拡大)、
//   R(θ) = 表示中心まわりの回転(cos_t/sin_t)。回転で窓の隅がキャンバス外に
//   出るぶん(canvas_uv が [0,1] の外)は BG_COLOR で塗る(下の fs_main 末尾)。
struct ViewUniform {
    center: vec2f,
    span: f32,
    cos_t: f32,
    sin_t: f32,
    // f32 スカラーで詰める(vec3f は align 16 で 48B に膨らみ Rust 側 32B と食い違う)
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};
// キャンバス外(回転・パンではみ出した領域)を塗る背景色(紙の周りの机)
const BG_COLOR: vec3f = vec3f(0.16, 0.16, 0.17);
@group(0) @binding(11) var<uniform> view: ViewUniform;
// アクティブタイル(M6): 各タイルの状態(TILE_COLD / TILE_COPY / TILE_ACTIVE)。表示モード 7 の可視化に使う
@group(0) @binding(12) var<storage, read> tile_active: array<u32>;

// 黒 → 青 → シアン → 白 のヒートマップ
fn heatmap(x: f32) -> vec3f {
    let t = clamp(x, 0.0, 1.0);
    let c0 = vec3f(0.0, 0.0, 0.0);
    let c1 = vec3f(0.1, 0.25, 0.8);
    let c2 = vec3f(0.1, 0.75, 0.9);
    let c3 = vec3f(1.0, 1.0, 1.0);
    if (t < 0.33) {
        return mix(c0, c1, t / 0.33);
    }
    if (t < 0.66) {
        return mix(c1, c2, (t - 0.33) / 0.33);
    }
    return mix(c2, c3, (t - 0.66) / 0.34);
}

fn hsv2rgb(h: f32, s: f32, v: f32) -> vec3f {
    let k = (vec3f(5.0, 3.0, 1.0) + h * 6.0) % vec3f(6.0);
    return v - v * s * clamp(min(k, 4.0 - k), vec3f(0.0), vec3f(1.0));
}

// mixbox の latent → RGB 多項式(eval_polynomial)の WGSL 移植。
// (c) 2022 Secret Weapons, CC BY-NC 4.0(mixbox クレート同梱の係数をそのまま使用)
fn mixbox_eval(z: vec4f) -> vec3f {
    let c0 = z.x;
    let c1 = z.y;
    let c2 = z.z;
    let c3 = z.w;
    let c00 = c0 * c0;
    let c11 = c1 * c1;
    let c22 = c2 * c2;
    let c33 = c3 * c3;
    let c01 = c0 * c1;
    let c02 = c0 * c2;
    let c12 = c1 * c2;

    var rgb = vec3f(0.0);
    rgb += (c0 * c00) * vec3f(0.07717053, 0.02826978, 0.24832992);
    rgb += (c1 * c11) * vec3f(0.95912302, 0.80256528, 0.03561839);
    rgb += (c2 * c22) * vec3f(0.74683774, 0.04868586, 0.00000000);
    rgb += (c3 * c33) * vec3f(0.99518138, 0.99978149, 0.99704802);
    rgb += (c00 * c1) * vec3f(0.04819146, 0.83363781, 0.32515377);
    rgb += (c01 * c1) * vec3f(-0.68146950, 1.46107803, 1.06980936);
    rgb += (c00 * c2) * vec3f(0.27058419, -0.15324870, 1.98735057);
    rgb += (c02 * c2) * vec3f(0.80478189, 0.67093710, 0.18424500);
    rgb += (c00 * c3) * vec3f(-0.35031003, 1.37855826, 3.68865000);
    rgb += (c0 * c33) * vec3f(1.05128046, 1.97815239, 2.82989073);
    rgb += (c11 * c2) * vec3f(3.21607125, 0.81270228, 1.03384539);
    rgb += (c1 * c22) * vec3f(2.78893374, 0.41565549, -0.04487295);
    rgb += (c11 * c3) * vec3f(3.02162577, 2.55374103, 0.32766114);
    rgb += (c1 * c33) * vec3f(2.95124691, 2.81201112, 1.17578442);
    rgb += (c22 * c3) * vec3f(2.82677043, 0.79933038, 1.81715262);
    rgb += (c2 * c33) * vec3f(2.99691099, 1.22593053, 1.80653661);
    rgb += (c01 * c2) * vec3f(1.87394106, 2.05027182, -0.29835996);
    rgb += (c01 * c3) * vec3f(2.56609566, 7.03428198, 0.62575374);
    rgb += (c02 * c3) * vec3f(4.08329484, -1.40408358, 2.14995522);
    rgb += (c12 * c3) * vec3f(6.00078678, 2.55552042, 1.90739502);
    return rgb;
}

// 濃度 → 発色の共通部(M1c/M2): 4顔料の濃度を被覆率に変換し、
// 下地(base = 紙 or 白の latent ペア)と latent 空間で混合してから RGB へ戻す。
// 「黄+青の境界が濁らず緑に馴染む」のはこの latent 混合が作る。
// pal = 使う顔料パレット枠(乾燥レイヤーはスロット別に記録した色、湿レイヤーは LIVE_PALETTE)
fn render_conc(conc_in: vec4f, z_base: vec4f, z_base_res: vec4f, pal: u32) -> vec3f {
    let conc = max(conc_in, vec4f(0.0));
    let total = conc.x + conc.y + conc.z + conc.w;
    // 被覆率: 濃度が上がるほど下地が見えなくなる(Beer-Lambert 風の飽和)
    let cover = 1.0 - exp(-params.pigment_density * total);

    // 下地(重み 1-cover)と各顔料(濃度比 × cover)の latent を線形混合
    var z_mix = (1.0 - cover) * z_base;
    var z_res = (1.0 - cover) * z_base_res;
    if (total > 1e-6) {
        let w = conc * (cover / total);
        let pb = pal_base(pal);
        for (var i = 0u; i < 4u; i++) {
            z_mix += w[i] * latents[pb + 2u * i];
            z_res += w[i] * latents[pb + 2u * i + 1u];
        }
    }
    return clamp(mixbox_eval(z_mix) + z_res.xyz, vec3f(0.0), vec3f(1.0));
}

// 湿レイヤーの通常レンダリング: 紙の上に発色させる(conc = 浮遊+沈着)
fn render_paper(water: f32, conc: vec4f) -> vec3f {
    var color = render_conc(conc, latents[0], latents[1], LIVE_PALETTE);
    // 濡れている場所をわずかに暗く(水だけのストロークも通常表示で見えるように)
    color *= 1.0 - 0.12 * clamp(water, 0.0, 1.0);
    return color;
}

// 可視な乾燥レイヤー(M2)の multiply 係数(キャッシュから)。顔料なしのテクセルは白(=1)
fn dried_factor(p: vec2f) -> vec3f {
    return load_bilinear(dried_cache, p).rgb;
}

// ---- KM 合成(M3): 層間の光学スタック。計算はリニア色空間 ----
fn srgb_to_linear(c: vec3f) -> vec3f {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3f(2.4));
    return select(lo, hi, c > vec3f(0.04045));
}

fn linear_to_srgb(c: vec3f) -> vec3f {
    let x = clamp(c, vec3f(0.0), vec3f(1.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3f(1.0 / 2.4)) - 0.055;
    return select(lo, hi, x > vec3f(0.0031308));
}

// 濃度 conc の1層を、リニア反射率 r_below の下地の上に KM で重ねた反射率(リニア)。
// 層の反射率 R=R_b(黒地発色)・透過率² T²=(R_w−R_b)(1−R_b)(crates/km/src/lib.rs の簡約)を
// 白地/黒地の mixbox 発色から求め、合成式 R = R_top + T²·R_below/(1−R_top·R_below) を畳む。
fn km_over_conc(conc: vec4f, r_below: vec3f, pal: u32) -> vec3f {
    let rw = srgb_to_linear(render_conc(conc, latents[2], latents[3], pal));
    let rb = srgb_to_linear(render_conc(conc, latents[4], latents[5], pal));
    let r_top = rb;
    let t2 = max(rw - rb, vec3f(0.0)) * (vec3f(1.0) - rb);
    let denom = max(vec3f(1.0) - r_top * r_below, vec3f(1e-4));
    return r_top + t2 * r_below / denom;
}

// 紙(最下層)→ 乾燥レイヤー(下から)→ 湿レイヤー(最上)を KM で光学合成した最終色(sRGB)。
// 紙+乾燥レイヤーまでの反射率はキャッシュ(dried_cache_main)から読む
fn km_composite(water: f32, conc: vec4f, p: vec2f) -> vec3f {
    var r = load_bilinear(dried_cache, p).rgb;
    // 湿レイヤーを最上に重ねる(現行パレット)
    r = km_over_conc(conc, r, LIVE_PALETTE);
    var color = linear_to_srgb(r);
    // 濡れている場所をわずかに暗く(render_paper と同じ演出。水だけのストロークも見えるように)
    color *= 1.0 - 0.12 * clamp(water, 0.0, 1.0);
    return color;
}

// 通常表示の最終合成: compose_mode で multiply(M2) か KM(M3) を選ぶ
fn compose(water: f32, conc: vec4f, p: vec2f) -> vec3f {
    if (params.compose_mode == 1u) {
        return km_composite(water, conc, p);
    }
    return render_paper(water, conc) * dried_factor(p);
}

// 湿レイヤーの顔料濃度(浮遊+沈着)を、縁のアンチエイリアス(edge_aa_conc)を掛けてからバイリニア補間する。
// 補間する4テクセルそれぞれに3×3近傍が要るので、4×4 ブロックを1回だけ読んで使い回す
fn wet_conc_aa(p: vec2f) -> vec4f {
    if (params.edge_aa <= 0.0) {
        return load_bilinear(susp_tex, p) + load_bilinear(dep_tex, p);
    }
    let q = p - 0.5;
    let base = floor(q);
    let f = q - base;
    let i0 = vec2i(base) - vec2i(1);
    var blk: array<vec4f, 16>;
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            let ip = i0 + vec2i(x, y);
            blk[y * 4 + x] = load_clamped(susp_tex, ip) + load_clamped(dep_tex, ip);
        }
    }
    var corner: array<vec4f, 4>;
    for (var k = 0; k < 4; k++) {
        let ox = k & 1;
        let oy = k >> 1u;
        var nb: array<vec4f, 9>;
        for (var dy = 0; dy < 3; dy++) {
            for (var dx = 0; dx < 3; dx++) {
                nb[dy * 3 + dx] = blk[(oy + dy) * 4 + ox + dx];
            }
        }
        corner[k] = edge_aa_conc(nb, params.pigment_density, params.edge_aa);
    }
    return mix(mix(corner[0], corner[1], f.x), mix(corner[2], corner[3], f.x), f.y);
}

// 線画(M4.5a)を色の上に重ねる。docs/design.md §3.5 の合成順(紙→乾燥→湿→線画→ハイライト)に従い、
// 下書き(鉛筆=グレー)→ 清書(ペン=濃色)の順にアルファ合成する。インク濃度 r がアルファ。
// 表示切替(show_pencil / show_pen)で各レイヤーを消せる
fn apply_lines(color_in: vec3f, p: vec2f) -> vec3f {
    var color = color_in;
    let pencil = load_bilinear(pencil_tex, p).r;
    let pen = load_bilinear(pen_tex, p).r;
    color = mix(color, vec3f(0.42, 0.42, 0.44), pencil * f32(params.show_pencil));
    color = mix(color, vec3f(0.08, 0.08, 0.10), pen * f32(params.show_pen));
    // ハイライト(M4.5c): 不透明白を最上段に重ねる(合成順 紙→乾燥→湿→線画→ハイライト)
    let hi = load_bilinear(highlight_tex, p).r;
    color = mix(color, vec3f(1.0), clamp(hi, 0.0, 1.0) * f32(params.show_highlight));
    return color;
}

// 乾燥レイヤーの合成キャッシュを焼く(1 invocation = 1 テクセル)。
// compose_mode=0: 可視な各レイヤーを白地に置いた透過率の積(色はレイヤーの記録スロットの顔料)。
// compose_mode=1: 紙の反射率(リニア)から下の層順に KM で重ねた反射率
@compute @workgroup_size(8, 8)
fn dried_cache_main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(dried_cache_out);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let ip = vec2i(gid.xy);
    let km = select(params.compose_mode == 1u, layers.force_mode == 2u, layers.force_mode != 0u);
    // 合成の出発点: 下地があればそれ、なければ紙(KM は紙の反射率。render_conc(0,紙) は紙色そのもの
    // で pal は濃度0なので不問 / 乗算は白=1)
    var acc = vec3f(1.0);
    if (layers.has_base != 0u) {
        acc = textureLoad(base_tex, ip, select(0, 1, km), 0).rgb;
    } else if (km) {
        acc = srgb_to_linear(render_conc(vec4f(0.0), latents[0], latents[1], LIVE_PALETTE));
    }
    for (var k = 0u; k < min(layers.count, MAX_LAYERS); k++) {
        if ((layers.visible_mask & (1u << k)) == 0u) {
            continue;
        }
        let slot = layers.order[k >> 2u][k & 3u];
        let conc = textureLoad(dried_tex, ip, slot, 0) * layers.density[k >> 2u][k & 3u];
        if (km) {
            acc = km_over_conc(conc, acc, slot);
        } else {
            // 白地(latents[2],[3])に置いた透過率
            acc *= render_conc(conc, latents[2], latents[3], slot);
        }
    }
    textureStore(dried_cache_out, ip, vec4f(acc, 1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(water_tex));
    // M6: 画面 uv をビューポート変換(パン/ズーム/回転)でキャンバス uv に写してから
    // テクセル座標へ。以降の全サンプル(水/浮遊/沈着/紙/乾燥/線画)がこの p を共有する。
    //   canvas_uv = center + R(θ)·(uv − 0.5)·span
    let d = in.uv - vec2f(0.5);
    let dr = vec2f(d.x * view.cos_t - d.y * view.sin_t,
                   d.x * view.sin_t + d.y * view.cos_t);
    let cuv = view.center + dr * view.span;
    // 回転・パンでキャンバス外に出たテクセルは背景色(紙の周りの机)で塗る
    let outside = any(cuv < vec2f(0.0)) || any(cuv > vec2f(1.0));
    let p = cuv * dims;
    let cell = load_bilinear(water_tex, p);
    let susp = load_bilinear(susp_tex, p);
    let dep = load_bilinear(dep_tex, p);
    let water = cell.r;
    let vel = cell.gb;

    var color: vec3f;
    switch params.display_mode {
        case 1u: {
            // 水量ヒートマップ
            color = heatmap(water * params.display_gain);
        }
        case 2u: {
            // 速度場: 方向を色相、大きさを明度に
            let speed = length(vel) * params.display_gain;
            let hue = atan2(vel.y, vel.x) / 6.2831853 + 0.5;
            color = hsv2rgb(hue, 1.0, clamp(speed, 0.0, 1.0));
        }
        case 3u: {
            // 湿りオーバーレイ: 通常表示の上に濡れ領域(wet-area mask)を青く重ねる
            color = apply_lines(compose(water, wet_conc_aa(p), p), p);
            if (is_wet(cell)) {
                color = mix(color, vec3f(0.15, 0.35, 0.95), 0.3);
            }
        }
        case 4u: {
            // 浮遊顔料ヒートマップ(水の流れに乗って動く分。4顔料の合計)
            color = heatmap(dot(susp, vec4f(1.0)) * params.display_gain);
        }
        case 5u: {
            // 沈着顔料ヒートマップ(紙に定着した分。4顔料の合計)
            color = heatmap(dot(dep, vec4f(1.0)) * params.display_gain);
        }
        case 6u: {
            // 紙ハイト(M1d): 白=山 / 黒=谷
            let h = load_bilinear(paper_tex, p).r;
            color = vec3f(clamp(h * params.display_gain, 0.0, 1.0));
        }
        case 7u: {
            // アクティブタイル(M6): 通常表示にタイル格子を重ね、計算中のタイルを緑、
            // コピーだけのタイルを青、何もしないタイルを暗く沈める。コストが濡れ面積比例かを目視できる
            color = apply_lines(compose(water, wet_conc_aa(p), p), p);
            let tp = vec2u(clamp(p, vec2f(0.0), dims - 1.0));
            let tile = tile_active[tile_index_of(tp)];
            if (tile == TILE_ACTIVE) {
                color = mix(color, vec3f(0.15, 0.95, 0.30), 0.22);
            } else if (tile == TILE_COPY) {
                color = mix(color * 0.6, vec3f(0.15, 0.35, 0.95), 0.25);
            } else {
                color *= 0.45;
            }
            // タイル境界にうっすら格子線(TILE_SIZE テクセルごと)
            let inpx = tp % TILE_SIZE;
            if (inpx.x == 0u || inpx.y == 0u) {
                color = mix(color, vec3f(0.0), 0.35);
            }
        }
        default: {
            // 通常: multiply(M2) か KM(M3) で湿+乾燥レイヤーを合成(params.compose_mode)し、
            // その上に線画(M4.5a)を重ねる
            color = apply_lines(compose(water, wet_conc_aa(p), p), p);
        }
    }
    // キャンバス外(回転・パンのはみ出し)は背景色で塗りつぶす
    return vec4f(select(color, BG_COLOR, outside), 1.0);
}
