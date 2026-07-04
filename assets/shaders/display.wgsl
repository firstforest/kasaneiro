// display.wgsl — シミュレーションテクスチャを egui のキャンバス領域に表示する。
// H4(デバッグ表示切替): params.display_mode で分岐する。
//   0 = 通常(顔料を紙の上にレンダリング。濡れている場所はわずかに暗く)
//   1 = 水量ヒートマップ
//   2 = 速度場(色相=方向、明度=大きさ)
//   3 = 湿りオーバーレイ(Rebelle の Show Wet 相当: 通常表示に濡れ領域を青重ね)
//   4 = 浮遊顔料ヒートマップ
//   5 = 沈着顔料ヒートマップ
// 先頭に common.wgsl が連結される。

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

// 通常レンダリング: 紙の白から顔料の吸収で減衰(Beer-Lambert 風)。
// 単顔料(r チャンネル)をフタロブルー風の吸収係数で色にする。M1c で mixbox 混色に置換。
fn render_paper(water: f32, pigment: f32) -> vec3f {
    let paper = vec3f(0.96, 0.95, 0.91);
    let absorb = vec3f(2.2, 0.9, 0.25);
    var color = paper * exp(-absorb * max(pigment, 0.0));
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
    // 見た目の色は浮遊+沈着の合計(M1b は r = 単顔料のみ)
    let pigment = susp.r + dep.r;

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
            color = render_paper(water, pigment);
            if (is_wet(cell)) {
                color = mix(color, vec3f(0.15, 0.35, 0.95), 0.3);
            }
        }
        case 4u: {
            // 浮遊顔料ヒートマップ(水の流れに乗って動く分)
            color = heatmap(susp.r * params.display_gain);
        }
        case 5u: {
            // 沈着顔料ヒートマップ(紙に定着した分)
            color = heatmap(dep.r * params.display_gain);
        }
        default: {
            // 通常
            color = render_paper(water, pigment);
        }
    }
    return vec4f(color, 1.0);
}
