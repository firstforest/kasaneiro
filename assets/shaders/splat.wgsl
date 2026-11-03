// splat.wgsl — ブラシ入力の compute パス。ツール(params.tool)で4分岐する:
//   0 = 描画: 筆の穂先(筆バッファ binding 12)と紙が濡れ具合の差で水を受け渡す(brush_exchange)。穂先が濡れて
//       いれば紙へ出て(顔料は穂先の濃さで一緒に出る)、紙が濡れていれば穂先へ入る(浮遊顔料は紙の濃さで一緒に入る)。
//       受け渡した量は binding 13 に集計し、brush.wgsl がフレームの最後に穂先へ反映して腹から補充する。
//       置いた水は毛細管拡散(diffuse.wgsl、note/05)が濡れた紙を伝ってひとりでに広げ、色の溶かし戻し
//       (paint_pickup)で下の色と馴染む。水だけの筆(顔料 0)なら水を置いて下の色を溶かすので、
//       先濡らしとぼかしもこの分岐が担う。空の筆(洗った直後、水 0)は紙に何も置かず、濡れた所の色水を吸うだけ
//   1 = リフト(削り): その場を濡らして沈着顔料を浮遊層へ戻し、浮いた色ごと水を吸い取る
//       (濡らして押さえる)。ステイニング顔料(ω)は残り、紙の凸部から先に剥がれる
//       (粒状化 γ の顔料は谷に残る) — Curtis の削りレシピ+吸い取り
//   2 = 消去: 湿レイヤーの水・速度・顔料・濡れマスクを機械的にゼロへ(紙の白まで戻す完全消去)
//   3 = 吸い取り: 乾いた筆(thirsty brush)。表面の自由水と、水に浮いている顔料を同率で
//       取り除く(absorb_rate)。沈着顔料は紙に付いているので残る = リフトと逆方向の対
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
// アクティブタイル(M6): タイルの状態(TILE_COLD / TILE_COPY / TILE_ACTIVE。tiledilate.wgsl 参照)
@group(0) @binding(11) var<storage, read> tile_active: array<u32>;
// 塗る筆の中身(フレーム頭の値。このパスでは穂先を読むだけ)と、穂先と紙が受け渡した量の集計
@group(0) @binding(12) var<storage, read_write> brush: Brush;
@group(0) @binding(13) var<storage, read_write> flow: BrushFlow;

// 4スロットの顔料個性 ω(染みつき)・γ(粒状)をベクトルにまとめる
fn pigment_omega() -> vec4f {
    return vec4f(pigment[0].y, pigment[1].y, pigment[2].y, pigment[3].y);
}

fn pigment_gamma() -> vec4f {
    return vec4f(pigment[0].z, pigment[1].z, pigment[2].z, pigment[3].z);
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

    let p = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_water, ip, 0);
    var susp = textureLoad(src_susp, ip, 0);
    var dep = textureLoad(src_dep, ip, 0);
    var water = cell.r;
    var vel = cell.gb;
    var wet = cell.a;
    let h = textureLoad(paper_tex, ip, 0).r;
    // このセルと筆が受け渡した量(筆へ入った向きが正。水, 4色の顔料)。最後にまとめて集計へ足す
    var flow_water = 0.0;
    var flow_pigment = vec4f(0.0);

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
            // 濡れマスクを落とすセルは乾いたセル扱いになり残りが居座るので、
            // keep で残る分(最大10%)も含めてゼロにする
            if (coverage > 0.9) {
                wet = 0.0;
                water = 0.0;
                vel = vec2f(0.0);
                susp = vec4f(0.0);
                dep = vec4f(0.0);
            }
        } else if (params.tool == 1u) {
            // リフト(削り): 再湿潤して沈着顔料を浮遊層へ戻し、下で吸い取る。
            // ステイニング ω が大きいほど (1−ω) で剥がれず床が残る。
            // 紙の凸部(h→1)ほど剥がれ、粒状顔料(γ大)は凹部に残る = 縁/谷に色が残る
            water += coverage * params.brush_water * mix(1.0, press, params.pressure_water);
            if (dist < radius) {
                wet = 1.0;
            }
            // (vec4 の成分を動的インデックスで書くと FXC が外側ループの展開を試みて失敗するので、
            //  4スロットまとめてベクトル演算で書く)
            let peak_gate = max(vec4f(1.0) + pigment_gamma() * (2.0 * h - 1.0), vec4f(0.0));
            let frac = clamp(params.lift_strength * coverage * press * (vec4f(1.0) - pigment_omega()) * peak_gate, vec4f(0.0), vec4f(1.0));
            let lifted = dep * frac;
            dep -= lifted;
            susp += lifted;
            // 押さえて吸い取る(ティッシュで押さえる工程): 浮かせた色ごと水を取り去る。
            // 浮かせるだけだと色は周りへ移るだけなので、吸い取ってその場から色を抜く
            // (水だけの筆=溶かして馴染ませる、との違い)。吸い取り tool=3 と同じ式
            let blot = clamp(params.lift_blot * coverage * press, 0.0, 1.0);
            water *= 1.0 - blot;
            vel *= 1.0 - blot;
            susp *= 1.0 - blot;
        } else if (params.tool == 3u) {
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
        } else {
            // 描画(既定): 穂先と紙が濡れ具合の差で水を受け渡す。濡れ具合は穂先が s_b = 穂先の水(1 = 満タン)、
            // 紙が s_p = 水位 ÷ 満タン水位(w_sat = brush_water × SAT_RATIO)。1 splat で動く水は
            // f = brush_exchange × coverage × 筆圧 × (s_b − s_p) × w_sat。
            // 穂先の水は s.scale(フレーム内で穂先が最も減った場合の残りの予測。CPU の BrushFrame::drag)を
            // 掛けた値を使うので、フレームの中で穂先に残っている以上を出すことはない。
            // 筆の中で混ざった色はそのまま4スロットへ同時に出る。紙の上で色が分かれるのは
            // 受け渡しの仕事ではなく、顔料個性の差(粒の細かさ μ・密度 ρ・染みつき ω)が起こす物理。
            // s.feed > 0.5 は置いたまま動かない筆の splat(CPU が毎フレーム積む)。式は同じで、
            // 紙と釣り合うまで出続けた水を毛細管拡散(diffuse.wgsl)が外へ運ぶので、
            // 「筆に含まれた色水が置いている間ずっと流れ出て広がる」動きになる。進行方向を持たない(s.vel = 0)ので、
            // 出た水は止まっていて初速を与えない。
            // 空の筆(洗った直後)は乾いた穂先として、濡れた所の色水を吸うだけ(紙へは何も出さない。
            // 乾いた紙を濡らさず、初速も溶かし戻しも与えない)
            let feed = s.feed > 0.5;
            let s_b = max(brush.tip_water * s.scale, 0.0);
            let empty = s_b <= 0.0;
            let w_sat = params.brush_water * SAT_RATIO;
            if (w_sat > 0.0 && coverage > 0.0) {
                let rate = clamp(params.brush_exchange * coverage * mix(1.0, press, params.pressure_water), 0.0, 1.0);
                let f = rate * (s_b - water / w_sat) * w_sat;
                if (f > 0.0) {
                    // 穂先 → 紙: 顔料は穂先の濃さ(顔料 ÷ 水を紙の単位へ直したもの)で一緒に出る。
                    // 筆圧(pressure_pigment)が弱いほど顔料を控えめに出す(出した分だけ穂先から引くので量は釣り合う)
                    let conc = brush.tip_pigment * (params.brush_pigment / (params.brush_water * brush.tip_water));
                    let out_pigment = f * max(conc, vec4f(0.0)) * mix(1.0, press, params.pressure_pigment);
                    // 初速は穂先から出た水が持ち込む: 紙の水と運動量で混ぜる。筆の勢いで色水が走るのは
                    // 筆が水を出したときだけで、吸っている筆は紙の水を押さない。出る水の速さは穂先の
                    // 濡れ具合の3乗: 満タンの筆は筆の勢いのまま、水の少ない筆(乾きかけ・拭った筆)ほど急に弱める。
                    // 少ない水が勢いを持つと、1歩ごとの濡れた所の前縁へ寄って粒の列を残すため
                    // (濡れ具合 0.3 で、比例では粒が見え、2乗でもうっすら残る)。
                    // 足し込みにしないので、1フレームに来る splat の数で勢いが変わらない
                    let v_brush = params.brush_velocity * s_b * s_b * s_b * s.vel;
                    vel = (vel * water + v_brush * f) / (water + f);
                    water += f;
                    susp += out_pigment;
                    flow_water -= f;
                    flow_pigment -= out_pigment;
                } else if (f < 0.0 && params.brush_hold > 0.0 && params.brush_tip > 0.0) {
                    // 紙 → 穂先: 浮遊顔料は水に乗っているので、吸った水の濃さ = セルの濃さ。
                    // 満タンに近い穂先ほど入る余地がないので、穂先の渇き(1 − s_b)を掛ける。
                    // 減らない筆(brush_hold = 0)は容量が無限なので吸わない
                    let take = -f * clamp(1.0 - s_b, 0.0, 1.0);
                    let ratio = take / water;
                    let taken = susp * ratio;
                    water -= take;
                    vel *= 1.0 - ratio;
                    susp -= taken;
                    flow_water += take;
                    flow_pigment += taken;
                }
            }
            if (empty) {
                continue;
            }
            // 筆が届いた範囲を濡らす(wet-area mask)。水が動けるのはこの領域だけ。
            // 水を置く範囲(coverage > 0)と一致させ、マスク外に水が取り残されないようにする
            if (dist < radius) {
                wet = 1.0;
            }
            // 色の溶かし戻し: 筆が触れた沈着顔料を浮遊層へ戻す(リフト tool=1 の弱い版)。
            // 筆の色と下の色が同じ浮遊層で混ざるので、置いた境界が濁らず馴染む。
            // ステイニング顔料(ω 大)は (1−ω) で剥がれず残る。
            // 置いたままの splat は毎フレーム来るので弱める(そのままだと1秒で筆の下が全部剥がれる)
            if (params.paint_pickup > 0.0 && coverage > 0.0) {
                let pickup_k = params.paint_pickup * select(1.0, 0.1, feed);
                let frac = clamp(pickup_k * coverage * press * (vec4f(1.0) - pigment_omega()), vec4f(0.0), vec4f(1.0));
                let picked = dep * frac;
                dep -= picked;
                susp += picked;
            }
        }
    }

    // 受け渡した量を集計へ足す(brush.wgsl が筆の中身へ移す)。足し算の順序に依らない整数で集計する
    if (flow_water != 0.0) {
        atomicAdd(&flow.amount[0], i32(round(flow_water * FLOW_SCALE)));
        let q = vec4i(round(flow_pigment * FLOW_SCALE));
        atomicAdd(&flow.amount[1], q.x);
        atomicAdd(&flow.amount[2], q.y);
        atomicAdd(&flow.amount[3], q.z);
        atomicAdd(&flow.amount[4], q.w);
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
