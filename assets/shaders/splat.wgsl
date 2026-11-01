// splat.wgsl — ブラシ入力の compute パス。ツール(params.tool)で4分岐する:
//   0 = 描画: 筆の中身(splat が運ぶ水と4色の顔料)を置く(M1a/M1b)。置いた水は毛細管拡散(diffuse.wgsl、note/05)が
//       濡れた紙を伝ってひとりでに広げ、色の溶かし戻し(paint_pickup)で下の色と馴染む。
//       水だけの筆(顔料 0)なら水を置いて下の色を溶かすので、先濡らしとぼかしもこの分岐が担う。
//       空の筆(水 0)は何もしない
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

// 筆の含み(brush_charge)の調整済み定数(docs/note/05 §4)。再調整はホットリロード(H1)で直接編集
const CHARGE_PIGMENT: f32 = 0.15; // feed splat 1フレームが注ぐ顔料の通常 splat に対する比

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
            // 描画(既定): 水+初速+顔料の注入。量は splat が運ぶ筆の中身(s.water / s.pigment、
            // 単位は満タン・標準の1回分)に brush_water / brush_pigment を掛けたもの。
            // 筆の中で混ざった色はそのまま4スロットへ同時に注ぐ。紙の上で色が分かれるのは
            // 注入の仕事ではなく、顔料個性の差(粒の細かさ μ・密度 ρ・染みつき ω)が起こす物理。
            // s.feed > 0.5 は「筆の含み」splat(置いたまま動かない間、CPU が毎フレーム積む):
            // 一括注入(+=)を毎フレーム繰り返すと溢れるので、水は目標水位への max 補充
            // (なでても積み上がらない)、顔料は CHARGE_PIGMENT ぶんだけ注ぐ。
            // 補充され続ける水を毛細管拡散(diffuse.wgsl)が外へ運ぶので、
            // 「筆に含まれた色水が置いている間ずっと流れ出て広がる」動きになる
            // 空の筆(洗った直後)は濡らしも溶かし戻しもしない
            if (s.water <= 0.0) {
                continue;
            }
            let feed = s.feed > 0.5;
            let w_in = coverage * params.brush_water * s.water * mix(1.0, press, params.pressure_water);
            var pig = coverage * params.brush_pigment * mix(1.0, press, params.pressure_pigment);
            if (!feed) {
                water += w_in;
                vel += coverage * params.brush_velocity * s.vel;
            } else {
                water = max(water, w_in);
                pig *= CHARGE_PIGMENT;
            }
            susp += pig * max(s.pigment, vec4f(0.0));
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
                let frac = clamp(pickup_k * coverage * press * (vec4f(1.0) - pigment_omega()), vec4f(0.0), vec4f(1.0));
                let picked = dep * frac;
                dep -= picked;
                susp += picked;
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
