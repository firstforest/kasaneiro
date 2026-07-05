// display.wgsl — シミュレーションテクスチャを egui のキャンバス領域に表示する。
// H4(デバッグ表示切替): params.display_mode で分岐する。
//   0 = 通常(顔料を mixbox 混色で紙の上にレンダリング。濡れている場所はわずかに暗く)
//   1 = 水量ヒートマップ
//   2 = 速度場(色相=方向、明度=大きさ)
//   3 = 湿りオーバーレイ(Rebelle の Show Wet 相当: 通常表示に濡れ領域を青重ね)
//   4 = 浮遊顔料ヒートマップ(4チャンネル合計)
//   5 = 沈着顔料ヒートマップ(4チャンネル合計)
//   6 = 紙ハイト(グレースケール。白=山 / 黒=谷)
// 先頭に common.wgsl が連結される。
//
// 通常表示の混色(M1c): 4顔料の濃度(浮遊+沈着)と紙を mixbox の latent 空間で
// 線形混合し、latent → RGB 多項式で発色する。latent は CPU 側(src/pigment.rs)が
// mixbox LUT から顔料ごとに1回だけ計算して uniform で渡すため、GPU に LUT は不要。
//
// 乾燥レイヤー(M2): texture array の各スライス = 1 乾燥レイヤー(rgba = 4顔料濃度)。
// 合成は params.compose_mode で切替(H5 の再生で A/B 比較):
//   0 = multiply(M2): 最終色 = 湿レイヤーの紙上発色 × Π(可視な乾燥レイヤーを白地に置いた透過率)
//   1 = KM(M3): 各層を白地/黒地に置いた発色 R_w,R_b から反射率 R=R_b・透過率² T²=(R_w−R_b)(1−R_b)
//       を導き(km.rs の簡約)、紙を最下層に下から光学合成する。上に薄い層を重ねるほど下が透ける
//       「内側から光る」グレーズ挙動になる。計算はリニア色空間で行い最後に sRGB へ戻す(note 03 §3)。
// どちらも層「内」の混色は mixbox の latent 混合(render_conc)を通す(黄+青が濁らない M1c の芯)。

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
// mixbox latent(src/pigment.rs の latent_uniform と同レイアウト):
//   [2i] = 顔料 i の (c0..c3) / [2i+1] = 顔料 i の RGB 残差
//   [8],[9] = 紙色 / [10],[11] = 白 / [12],[13] = 黒(KM 合成の R_b 用)
@group(0) @binding(4) var<uniform> latents: array<vec4f, 14>;
@group(0) @binding(5) var paper_tex: texture_2d<f32>;
// 乾燥レイヤー(M2): スライス = レイヤースロット、rgba = 4顔料濃度
@group(0) @binding(6) var dried_tex: texture_2d_array<f32>;
// src/gpu/mod.rs の LayerUniform と同レイアウト。
// order[k] = 下から k 番目のレイヤーのスロット番号 / visible_mask の bit k = 下から k 番目の可視性
// (multiply は可換なので順序は今は見た目に効かないが、KM 合成(M3)で効くため配管だけ通しておく)
struct LayerUniform {
    count: u32,
    visible_mask: u32,
    _pad0: u32,
    _pad1: u32,
    order: array<vec4u, 2>,
};
@group(0) @binding(7) var<uniform> layers: LayerUniform;

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
fn render_conc(conc_in: vec4f, z_base: vec4f, z_base_res: vec4f) -> vec3f {
    let conc = max(conc_in, vec4f(0.0));
    let total = conc.x + conc.y + conc.z + conc.w;
    // 被覆率: 濃度が上がるほど下地が見えなくなる(Beer-Lambert 風の飽和)
    let cover = 1.0 - exp(-params.pigment_density * total);

    // 下地(重み 1-cover)と各顔料(濃度比 × cover)の latent を線形混合
    var z_mix = (1.0 - cover) * z_base;
    var z_res = (1.0 - cover) * z_base_res;
    if (total > 1e-6) {
        let w = conc * (cover / total);
        for (var i = 0u; i < 4u; i++) {
            z_mix += w[i] * latents[2u * i];
            z_res += w[i] * latents[2u * i + 1u];
        }
    }
    return clamp(mixbox_eval(z_mix) + z_res.xyz, vec3f(0.0), vec3f(1.0));
}

// 湿レイヤーの通常レンダリング: 紙の上に発色させる
fn render_paper(water: f32, susp: vec4f, dep: vec4f) -> vec3f {
    var color = render_conc(susp + dep, latents[8], latents[9]);
    // 濡れている場所をわずかに暗く(水だけのストロークも通常表示で見えるように)
    color *= 1.0 - 0.12 * clamp(water, 0.0, 1.0);
    return color;
}

// 乾燥レイヤー用の load_bilinear(texture_2d_array 版)
fn load_clamped_layer(t: texture_2d_array<f32>, p: vec2i, layer: u32) -> vec4f {
    let dims = vec2i(textureDimensions(t));
    return textureLoad(t, clamp(p, vec2i(0), dims - 1), layer, 0);
}

fn load_bilinear_layer(t: texture_2d_array<f32>, p: vec2f, layer: u32) -> vec4f {
    let q = p - 0.5;
    let base = floor(q);
    let f = q - base;
    let i = vec2i(base);
    let c00 = load_clamped_layer(t, i, layer);
    let c10 = load_clamped_layer(t, i + vec2i(1, 0), layer);
    let c01 = load_clamped_layer(t, i + vec2i(0, 1), layer);
    let c11 = load_clamped_layer(t, i + vec2i(1, 1), layer);
    return mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
}

// 可視な乾燥レイヤー(M2)の multiply 係数: 各レイヤーを白地に置いた透過率の積。
// 顔料なしのテクセルは白(=1)なので積に影響しない
fn dried_factor(p: vec2f) -> vec3f {
    var factor = vec3f(1.0);
    for (var k = 0u; k < min(layers.count, 8u); k++) {
        if ((layers.visible_mask & (1u << k)) == 0u) {
            continue;
        }
        let slot = layers.order[k >> 2u][k & 3u];
        let conc = load_bilinear_layer(dried_tex, p, slot);
        factor *= render_conc(conc, latents[10], latents[11]);
    }
    return factor;
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
// 層の反射率 R=R_b(黒地発色)・透過率² T²=(R_w−R_b)(1−R_b)(km.rs の簡約)を
// 白地/黒地の mixbox 発色から求め、合成式 R = R_top + T²·R_below/(1−R_top·R_below) を畳む。
fn km_over_conc(conc: vec4f, r_below: vec3f) -> vec3f {
    let rw = srgb_to_linear(render_conc(conc, latents[10], latents[11]));
    let rb = srgb_to_linear(render_conc(conc, latents[12], latents[13]));
    let r_top = rb;
    let t2 = max(rw - rb, vec3f(0.0)) * (vec3f(1.0) - rb);
    let denom = max(vec3f(1.0) - r_top * r_below, vec3f(1e-4));
    return r_top + t2 * r_below / denom;
}

// 紙(最下層)→ 乾燥レイヤー(下から)→ 湿レイヤー(最上)を KM で光学合成した最終色(sRGB)。
fn km_composite(water: f32, susp: vec4f, dep: vec4f, p: vec2f) -> vec3f {
    // 下地 = 紙の反射率(リニア)。render_conc(0,紙) は紙色そのもの
    var r = srgb_to_linear(render_conc(vec4f(0.0), latents[8], latents[9]));
    for (var k = 0u; k < min(layers.count, 8u); k++) {
        if ((layers.visible_mask & (1u << k)) == 0u) {
            continue;
        }
        let slot = layers.order[k >> 2u][k & 3u];
        let conc = load_bilinear_layer(dried_tex, p, slot);
        r = km_over_conc(conc, r);
    }
    // 湿レイヤーを最上に重ねる
    r = km_over_conc(susp + dep, r);
    var color = linear_to_srgb(r);
    // 濡れている場所をわずかに暗く(render_paper と同じ演出。水だけのストロークも見えるように)
    color *= 1.0 - 0.12 * clamp(water, 0.0, 1.0);
    return color;
}

// 通常表示の最終合成: compose_mode で multiply(M2) か KM(M3) を選ぶ
fn compose(water: f32, susp: vec4f, dep: vec4f, p: vec2f) -> vec3f {
    if (params.compose_mode == 1u) {
        return km_composite(water, susp, dep, p);
    }
    return render_paper(water, susp, dep) * dried_factor(p);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(water_tex));
    let p = in.uv * dims;
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
            color = compose(water, susp, dep, p);
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
        default: {
            // 通常: multiply(M2) か KM(M3) で湿+乾燥レイヤーを合成(params.compose_mode)
            color = compose(water, susp, dep, p);
        }
    }
    return vec4f(color, 1.0);
}
