// brush.wgsl — 塗る筆の中身(binding 12)をフレームの最後に1回だけ更新する compute パス(1スレッド)。
// splat.wgsl の後、塗るツールのフレームだけ走る。順に:
//   1. なぞった長さで減らす(splat_buf.brush_scale。CPU が距離から出す。水と顔料は同じ割合)
//   2. 置いたまま流れ出た色水の分、水を使う(splat_buf.feed_dt。満タンが brush_charge 秒で尽きる速さ)
//   3. 紙から吸った量(binding 13 の集計)を筆の単位に直して足し、集計を 0 に戻す
// 先頭に common.wgsl が連結される(SimParams / Splat / Brush / BrushIntake はそちら)。

@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;
@group(0) @binding(12) var<storage, read_write> brush: Brush;
@group(0) @binding(13) var<storage, read_write> intake: BrushIntake;

const PI: f32 = 3.14159265;
const LN2: f32 = 0.6931472;

@compute @workgroup_size(1)
fn main() {
    var water = brush.water * splat_buf.brush_scale;
    var pigment = brush.pigment * splat_buf.brush_scale;

    if (splat_buf.feed_dt > 0.0 && params.brush_charge > 0.0 && water > FEED_MIN_WATER) {
        let left = max(water - splat_buf.feed_dt / params.brush_charge, 0.0);
        pigment *= left / water;
        water = left;
    }

    let took_water = f32(atomicLoad(&intake.amount[0])) / INTAKE_SCALE;
    let took_pigment = vec4f(
        f32(atomicLoad(&intake.amount[1])),
        f32(atomicLoad(&intake.amount[2])),
        f32(atomicLoad(&intake.amount[3])),
        f32(atomicLoad(&intake.amount[4])),
    ) / INTAKE_SCALE;
    for (var i = 0u; i < 5u; i++) {
        atomicStore(&intake.amount[i], 0u);
    }

    // 吸った量はセルの量の合計なので、満タンの筆が一生で紙に置く量を 1 として筆の単位に直す。
    // 筆跡1つは満量で brush_water / brush_pigment ずつを被覆の面積に置き、筆はなぞって brush_hold で
    // 半分になる(= 中身が尽きるまでに brush_hold ÷ ln2 ÷ 間隔 個の筆跡を置く)ので、
    // 一生で置く量 = 筆跡1つの量 × その個数。置いた分以上は戻らない(紙と筆の間で量が釣り合う)。
    // 濃さ(顔料 ÷ 水)は紙の上と同じ比で筆に入る
    if (took_water > 0.0 && params.brush_hold > 0.0) {
        let r = max(params.brush_radius, 0.5);
        let footprint = FOOTPRINT_RATIO * PI * r * r;
        let life = params.brush_hold / (LN2 * splat_spacing());
        let gain_water = took_water / (max(params.brush_water, 1e-3) * footprint * life);
        let gain_pigment = took_pigment / (max(params.brush_pigment, 1e-3) * footprint * life);
        // 水は満タンまで。入りきらない分は、その水に浮いていた顔料も同じ割合で入らない
        let fit = min(1.0, max(1.0 - water, 0.0) / gain_water);
        water += gain_water * fit;
        pigment += gain_pigment * fit;
        let total = pigment.x + pigment.y + pigment.z + pigment.w;
        if (total > PIGMENT_MAX) {
            pigment *= PIGMENT_MAX / total;
        }
    }

    brush.water = water;
    brush.pigment = max(pigment, vec4f(0.0));
}
