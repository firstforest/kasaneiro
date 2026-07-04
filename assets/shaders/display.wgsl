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
//   [2i] = 顔料 i の (c0..c3) / [2i+1] = 顔料 i の RGB 残差 / [8],[9] = 紙色の latent
@group(0) @binding(4) var<uniform> latents: array<vec4f, 10>;
@group(0) @binding(5) var paper_tex: texture_2d<f32>;

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

// 通常レンダリング(M1c): 顔料濃度(浮遊+沈着の4チャンネル)を被覆率に変換し、
// 顔料同士と紙を mixbox latent 空間で混合してから RGB へ戻す。
// 「黄+青の境界が濁らず緑に馴染む」のはこの latent 混合が作る。
fn render_paper(water: f32, susp: vec4f, dep: vec4f) -> vec3f {
    let conc = max(susp + dep, vec4f(0.0));
    let total = conc.x + conc.y + conc.z + conc.w;
    // 被覆率: 濃度が上がるほど紙が見えなくなる(Beer-Lambert 風の飽和)
    let cover = 1.0 - exp(-params.pigment_density * total);

    // 紙(重み 1-cover)と各顔料(濃度比 × cover)の latent を線形混合
    var z_mix = (1.0 - cover) * latents[8];
    var z_res = (1.0 - cover) * latents[9];
    if (total > 1e-6) {
        let w = conc * (cover / total);
        for (var i = 0u; i < 4u; i++) {
            z_mix += w[i] * latents[2u * i];
            z_res += w[i] * latents[2u * i + 1u];
        }
    }
    var color = clamp(mixbox_eval(z_mix) + z_res.xyz, vec3f(0.0), vec3f(1.0));
    // 濡れている場所をわずかに暗く(水だけのストロークも通常表示で見えるように)
    color *= 1.0 - 0.12 * clamp(water, 0.0, 1.0);
    return color;
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
            color = render_paper(water, susp, dep);
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
            // 通常
            color = render_paper(water, susp, dep);
        }
    }
    return vec4f(color, 1.0);
}
