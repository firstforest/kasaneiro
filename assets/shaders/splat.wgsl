// splat.wgsl — ブラシ入力の compute パス。ツール(params.tool)で3分岐する(M3):
//   0 = 描画: 水+初速+顔料を置く(M1a/M1b)
//   1 = リフト(削り): 沈着顔料を浮遊層へ戻し、その場を濡らして流す。ステイニング顔料(ω)は
//       残り、紙の凸部から先に剥がれる(粒状化 γ の顔料は谷に残る) — Curtis の削りレシピ
//   2 = 消去: 湿レイヤーの水・速度・顔料・濡れマスクを機械的にゼロへ(紙の白まで戻す完全消去)
// 先頭に common.wgsl が連結される(SimParams / Splat / ヘルパー関数はそちら)。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var dst_water: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var dst_susp: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var src_dep: texture_2d<f32>;
@group(0) @binding(5) var dst_dep: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;
@group(0) @binding(8) var paper_tex: texture_2d<f32>;
// 顔料個性(M3): [i] = (密度 ρ, ステイニング ω, 粒状感 γ, 予備)。リフトの ω/γ 変調に使う
@group(0) @binding(9) var<uniform> pigment: array<vec4f, 4>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_water);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }

    let p = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_water, ip, 0);
    var susp = textureLoad(src_susp, ip, 0);
    var dep = textureLoad(src_dep, ip, 0);
    var water = cell.r;
    var vel = cell.gb;
    var wet = cell.a;
    let h = textureLoad(paper_tex, ip, 0).r;

    for (var i = 0u; i < splat_buf.count; i++) {
        let s = splat_buf.splats[i];
        // 筆圧マッピング(M1.5): 半径・水量・顔料量それぞれに「効き」スライダーで反映。
        // CPU 側の SimParams::radius_at と同じ式(サンプル間隔の算出に使う)
        let press = pressure_curve(s.pressure);
        let radius = max(params.brush_radius * mix(1.0, press, params.pressure_radius), 0.5);
        let dist = distance(p, s.pos);
        // 中心は満量、縁にかけて柔らかく減衰
        let coverage = 1.0 - smoothstep(radius * 0.6, radius, dist);

        if (params.tool == 2u) {
            // 消去: 中心ほど強く水・顔料・速度を削り、芯(coverage≈1)は紙の白まで戻す
            let keep = clamp(1.0 - coverage, 0.0, 1.0);
            water *= keep;
            vel *= keep;
            susp *= keep;
            dep *= keep;
            if (coverage > 0.9) {
                wet = 0.0;
            }
        } else if (params.tool == 1u) {
            // リフト(削り): 再湿潤して沈着顔料を浮遊層へ戻す(削除ではなく転送)。
            // ステイニング ω が大きいほど (1−ω) で剥がれず床が残る。
            // 紙の凸部(h→1)ほど剥がれ、粒状顔料(γ大)は凹部に残る = 縁/谷に色が残る
            water += coverage * params.brush_water * mix(1.0, press, params.pressure_water);
            if (dist < radius) {
                wet = 1.0;
            }
            for (var c = 0u; c < 4u; c++) {
                let omega = pigment[c].y;
                let gamma = pigment[c].z;
                let peak_gate = max(1.0 + gamma * (2.0 * h - 1.0), 0.0);
                let frac = clamp(params.lift_strength * coverage * press * (1.0 - omega) * peak_gate, 0.0, 1.0);
                let lifted = dep[c] * frac;
                dep[c] -= lifted;
                susp[c] += lifted;
            }
        } else {
            // 描画(既定): 水+初速+顔料の注入
            water += coverage * params.brush_water * mix(1.0, press, params.pressure_water);
            vel += coverage * params.brush_velocity * s.vel;
            // 顔料は浮遊層の選択チャンネル(brush_channel = パレットの顔料スロット)へ注入する
            susp[min(params.brush_channel, 3u)] += coverage * params.brush_pigment * mix(1.0, press, params.pressure_pigment);
            // 筆が届いた範囲を濡らす(wet-area mask)。水が動けるのはこの領域だけ。
            // 水を置く範囲(coverage > 0)と一致させ、マスク外に水が取り残されないようにする
            if (dist < radius) {
                wet = 1.0;
            }
        }
    }

    // 安定性: 水量は非負、速度は CFL 的上限でクランプ
    water = max(water, 0.0);
    let speed = length(vel);
    if (speed > params.vel_max) {
        vel *= params.vel_max / speed;
    }

    textureStore(dst_water, ip, vec4f(water, vel, wet));
    textureStore(dst_susp, ip, max(susp, vec4f(0.0)));
    textureStore(dst_dep, ip, max(dep, vec4f(0.0)));
}
