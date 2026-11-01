// brush.wgsl — 塗る筆の中身(binding 12。穂先と腹の2室)をフレームの最後に1回だけ更新する compute パス
// (1スレッド)。splat.wgsl の後、塗るツールで splat があるフレームだけ走る。
//   1. 穂先と紙が受け渡した量(binding 13 の集計。穂先へ入った向きが正、紙のセルの量の単位)を
//      穂先の容量で割って穂先に足し引きし、集計を 0 に戻す
//   2. 腹が穂先より濡れていれば、腹から穂先へ水を送る(一方向。顔料は腹の濃さで一緒に動く)。
//      濡れ具合の差を時定数 brush_refill_time で詰める。時間は splat_buf.brush_dt(前回の brush パスから
//      の経過。筆を上げていた間の分もまとめて来る)
// 量は保存する(紙へ出た分だけ穂先が減り、腹から出た分だけ穂先が増える)。
// 先頭に common.wgsl が連結される(SimParams / Splat / Brush / BrushFlow はそちら)。

@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;
@group(0) @binding(12) var<storage, read_write> brush: Brush;
@group(0) @binding(13) var<storage, read_write> flow: BrushFlow;

const LN2: f32 = 0.6931472;

// 「満タンの室だけで乾いた紙をこの長さ塗ると半分になる」長さ(px)から室の容量(紙のセルの量)を出す。
// 乾いた紙を塗ると、幅 CROSS_RATIO × r の帯が室の濡れ具合 × w_sat まで濡れるので、
// 中身は塗った長さに対して指数で減り、その長さで半分になる
fn capacity(half_length: f32, w_sat: f32) -> f32 {
    return half_length * CROSS_RATIO * max(params.brush_radius, 0.5) * w_sat / LN2;
}

// 室の中身を 0..満タンに収める(顔料は合計 PIGMENT_MAX で頭打ち)
fn clamp_water(w: f32) -> f32 {
    return clamp(w, 0.0, 1.0);
}

fn clamp_pigment(p: vec4f) -> vec4f {
    let q = max(p, vec4f(0.0));
    let total = q.x + q.y + q.z + q.w;
    if (total > PIGMENT_MAX) {
        return q * (PIGMENT_MAX / total);
    }
    return q;
}

@compute @workgroup_size(1)
fn main() {
    let flow_water = f32(atomicLoad(&flow.amount[0])) / FLOW_SCALE;
    let flow_pigment = vec4f(
        f32(atomicLoad(&flow.amount[1])),
        f32(atomicLoad(&flow.amount[2])),
        f32(atomicLoad(&flow.amount[3])),
        f32(atomicLoad(&flow.amount[4])),
    ) / FLOW_SCALE;
    for (var i = 0u; i < 5u; i++) {
        atomicStore(&flow.amount[i], 0);
    }

    // 減らない筆(brush_hold = 0)は容量が無限: 出しても減らない(吸いもしない。splat.wgsl)
    let w_sat = params.brush_water * SAT_RATIO;
    if (params.brush_hold <= 0.0 || params.brush_tip <= 0.0 || w_sat <= 0.0) {
        return;
    }
    let tip_cap = capacity(params.brush_tip, w_sat);
    let belly_cap = capacity(params.brush_hold, w_sat);
    // 顔料は「満タンの室の標準の1回分 = 紙の上で濃さ brush_pigment ÷ brush_water」になる単位
    let pigment_per_water = max(params.brush_pigment, 1e-6) / params.brush_water;

    // 1. 穂先と紙の受け渡し
    var tip_water = brush.tip_water + flow_water / tip_cap;
    var tip_pigment = brush.tip_pigment + flow_pigment / (tip_cap * pigment_per_water);
    // 安全弁: フレーム内の予測(CPU の BrushFrame::drag)で出しすぎは起きないはずだが、丸めの分などで
    // はみ出したら 0 で止める
    if (tip_water <= 0.0) {
        tip_water = 0.0;
        tip_pigment = vec4f(0.0);
    }

    // 2. 腹から穂先へ補充: 差を詰め切ったときに2室の濡れ具合が揃う量(紙のセルの量)を、
    // 経過時間の分だけ指数で詰める
    var belly_water = brush.belly_water;
    var belly_pigment = brush.belly_pigment;
    let gap = belly_water - tip_water;
    if (gap > 0.0 && params.brush_refill_time > 0.0 && splat_buf.brush_dt > 0.0) {
        let k = 1.0 - exp(-splat_buf.brush_dt / params.brush_refill_time);
        let moved = k * gap * tip_cap * belly_cap / (tip_cap + belly_cap);
        // 腹から出る割合(腹の水に対して)。顔料も同じ割合で腹から出て穂先へ入る
        let out_frac = min(moved / belly_cap / belly_water, 1.0);
        let moved_pigment = belly_pigment * out_frac;
        belly_water -= belly_water * out_frac;
        belly_pigment -= moved_pigment;
        tip_water += moved / tip_cap;
        tip_pigment += moved_pigment * (belly_cap / tip_cap);
    }

    brush.tip_water = clamp_water(tip_water);
    brush.tip_pigment = clamp_pigment(tip_pigment);
    brush.belly_water = clamp_water(belly_water);
    brush.belly_pigment = clamp_pigment(belly_pigment);
}
