// splat.wgsl — ブラシ入力の compute パス。ツール(params.tool)で3分岐する(M3):
//   0 = 描画: 水+初速+顔料を置く(M1a/M1b)。置いた水は毛細管拡散(diffuse.wgsl、note/07)が
//       濡れた紙を伝ってひとりでに広げ、色の溶かし戻し(paint_pickup)で下の色と馴染む
//   1 = リフト(削り): 沈着顔料を浮遊層へ戻し、その場を濡らして流す。ステイニング顔料(ω)は
//       残り、紙の凸部から先に剥がれる(粒状化 γ の顔料は谷に残る) — Curtis の削りレシピ
//   2 = 消去: 湿レイヤーの水・速度・顔料・濡れマスクを機械的にゼロへ(紙の白まで戻す完全消去)
//   3 = ぼかし筆: 顔料を注がず水を置き、ブラシ下の沈着顔料を浮遊層へ溶かし戻す(water_lift)。
//       馴染ませは毛細管拡散+γ重み顔料拡散の物理に任せる(2026-07-09 に箱ぼかしの均しを廃止して
//       一次原理化)
//   4 = 吸い取り: 乾いた筆(thirsty brush)。表面の自由水と、水に浮いている顔料を同率で
//       取り除く(absorb_rate)。沈着顔料は紙に付いているので残る = リフトと逆方向の対。
//       旧ならしの跡地(2026-07-09 廃止)を 2026-07-10 に再利用
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
// アクティブタイル(M6): タイル有効フラグ。非アクティブなタイルは素通しして計算を省く
@group(0) @binding(11) var<storage, read> tile_active: array<u32>;

// 筆の含み(brush_charge)の調整済み定数(docs/note/06)。再調整はホットリロード(H1)で直接編集
const CHARGE_PIGMENT: f32 = 0.15; // feed splat 1フレームが注ぐ顔料の通常 splat に対する比

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
        } else if (params.tool == 4u) {
            // 吸い取り(乾いた筆): 表面の自由水と、水に浮いている顔料を同じ割合で取り除く。
            // 浮遊顔料は水に乗っているだけなので、吸った水の顔料濃度=セルの濃度、
            // つまり水と同率で取れる(ω=染みつきは沈着の性質なのでここでは関与しない)。
            // 沈着顔料には触れない(剥がすのはリフト tool=1)。水が減るぶん運動量も
            // 持ち去られるので速度も同率で減衰。濡れマスクは立てたまま(紙はまだ湿っている
            // =周囲から毛細管で水が戻る「湿り戻り」も物理に任せる)
            let frac = clamp(params.absorb_rate * coverage * press, 0.0, 1.0);
            water *= 1.0 - frac;
            vel *= 1.0 - frac;
            susp *= 1.0 - frac;
        } else if (params.tool == 3u) {
            // ぼかし筆: 顔料・初速は注入せず、水を置いて下の色を溶かし戻す。
            //   用途1: 大きな領域を先に濡らす → 後で置いた顔料筆が毛細管拡散で滑らかに広がる
            //   用途2: 境界をなでる → 沈着顔料が浮遊層へ戻り、水量に応じた拡散でひとりでに馴染む
            // 水は「足す」のではなく目標水位まで「上げる」(max)。なでても水位が積み上がらない。
            let target_water = coverage * params.brush_water * mix(1.0, press, params.pressure_water);
            water = max(water, target_water);
            if (dist < radius) {
                wet = 1.0;
            }
            // 溶かし戻し: リフト(tool=1)の弱い版・paint_pickup と同じレシピ。
            // 紙ハイトのゲートは掛けない(ぼかしは均したい操作で、紙目の強調は不要)。
            // ステイニング ω の顔料は (1−ω) で剥がれず残る = 描線の芯は保たれる
            for (var c = 0u; c < 4u; c++) {
                let frac = clamp(params.water_lift * coverage * press * (1.0 - pigment[c].y), 0.0, 1.0);
                let picked = dep[c] * frac;
                dep[c] -= picked;
                susp[c] += picked;
            }
        } else {
            // 描画(既定): 水+初速+顔料の注入。
            // s.feed > 0.5 は「筆の含み」splat(置いたまま動かない間、CPU が毎フレーム積む):
            // 一括注入(+=)を毎フレーム繰り返すと溢れるので、水は目標水位への max 補充
            // (なでても積み上がらない。ぼかし筆と同じ流儀)、顔料は CHARGE_PIGMENT ぶんだけ注ぐ。
            // 補充され続ける水を毛細管拡散(diffuse.wgsl)が外へ運ぶので、
            // 「筆に含まれた色水が置いている間ずっと流れ出て広がる」動きになる
            let feed = s.feed > 0.5;
            if (!feed) {
                water += coverage * params.brush_water * mix(1.0, press, params.pressure_water);
                vel += coverage * params.brush_velocity * s.vel;
                // 顔料は浮遊層の選択チャンネル(brush_channel = パレットの顔料スロット)へ注入する
                susp[min(params.brush_channel, 3u)] += coverage * params.brush_pigment * mix(1.0, press, params.pressure_pigment);
            } else {
                water = max(water, coverage * params.brush_water * mix(1.0, press, params.pressure_water));
                susp[min(params.brush_channel, 3u)] += coverage * params.brush_pigment * CHARGE_PIGMENT
                    * mix(1.0, press, params.pressure_pigment);
            }
            // 筆が届いた範囲を濡らす(wet-area mask)。水が動けるのはこの領域だけ。
            // 水を置く範囲(coverage > 0)と一致させ、マスク外に水が取り残されないようにする
            if (dist < radius) {
                wet = 1.0;
            }
            // 色の溶かし戻し: 筆が触れた沈着顔料を浮遊層へ戻す(リフト tool=1 の弱い版)。
            // 筆の色と下の色が同じ浮遊層で混ざるので、置いた境界が濁らず馴染む。
            // ステイニング顔料(ω 大)は (1−ω) で剥がれず残る。
            // feed splat は毎フレーム来るので弱める(そのままだと1秒で筆の下が全部剥がれる)
            if (params.paint_pickup > 0.0 && coverage > 0.0) {
                let pickup_k = params.paint_pickup * select(1.0, 0.1, feed);
                for (var c = 0u; c < 4u; c++) {
                    let frac = clamp(pickup_k * coverage * press * (1.0 - pigment[c].y), 0.0, 1.0);
                    let picked = dep[c] * frac;
                    dep[c] -= picked;
                    susp[c] += picked;
                }
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
