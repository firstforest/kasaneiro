// display.wgsl — シミュレーションテクスチャを egui のキャンバス領域に表示する。
// H4(デバッグ表示切替): params.display_mode で分岐する。
//   0 = 通常(紙の上の水を色で可視化)
//   1 = 水量ヒートマップ
//   2 = 速度場(色相=方向、明度=大きさ)
//   3 = 湿りオーバーレイ(Rebelle の Show Wet 相当: 通常表示に濡れ領域を青重ね)
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

@group(0) @binding(0) var canvas_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: SimParams;

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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(canvas_tex));
    let cell = load_bilinear(canvas_tex, in.uv * dims);
    let water = cell.r * params.display_gain;
    let vel = cell.gb;

    var color: vec3f;
    switch params.display_mode {
        case 0u: {
            // 通常: 紙の白 → 水色。深いほど濃く(M1b で顔料表示に置き換わる)
            let paper = vec3f(0.96, 0.95, 0.91);
            let tint = vec3f(0.35, 0.55, 0.80);
            color = mix(paper, tint, clamp(water, 0.0, 0.85));
        }
        case 1u: {
            // 水量ヒートマップ
            color = heatmap(water);
        }
        case 3u: {
            // 湿りオーバーレイ: 通常表示の上に濡れ領域(wet-area mask)を青く重ねる
            let paper = vec3f(0.96, 0.95, 0.91);
            let tint = vec3f(0.35, 0.55, 0.80);
            color = mix(paper, tint, clamp(water, 0.0, 0.85));
            if (is_wet(cell)) {
                color = mix(color, vec3f(0.15, 0.35, 0.95), 0.3);
            }
        }
        default: {
            // 速度場: 方向を色相、大きさを明度に
            let speed = length(vel) * params.display_gain;
            let hue = atan2(vel.y, vel.x) / 6.2831853 + 0.5;
            color = hsv2rgb(hue, 1.0, clamp(speed, 0.0, 1.0));
        }
    }
    return vec4f(color, 1.0);
}
