// tilescan.wgsl — アクティブタイル(M6)の第1段: タイルごとの「生」有効フラグを作る。
// 1 invocation = 1タイル。タイル内にまだ動きうる濡れたセルがあるか、またはこのフレームのブラシ
// (流体へ流す splat)が近傍に触れるかを判定し、raw_active[タイル] に 0/1 を書く。
// このあと tiledilate.wgsl が1タイル分ふくらませて active を作る(にじみ前線の余裕)。
// シミュ本体より前(gpu/callback.rs)に current テクスチャを src として読む。
// active_tiles==0(最適化オフ)のときは全タイルを有効化して全面計算にする。
// 先頭に common.wgsl が連結される(SimParams / SplatBuffer / TILE_SIZE / TILES_PER_SIDE)。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: SimParams;
@group(0) @binding(4) var<storage, read> splat_buf: SplatBuffer;
@group(0) @binding(5) var<storage, read_write> raw_active: array<u32>;

// これ以下の浮遊顔料濃度は表示にも沈着量にも見えない
const SUSP_EPS: f32 = 1e-6;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    if (gid.x >= TILES_PER_SIDE || gid.y >= TILES_PER_SIDE) {
        return;
    }
    let idx = gid.y * TILES_PER_SIDE + gid.x;

    // 最適化オフ: 全タイル有効(gate は常に通る = 全面計算)
    if (params.active_tiles == 0u) {
        raw_active[idx] = 1u;
        return;
    }

    let x0 = gid.x * TILE_SIZE;
    let y0 = gid.y * TILE_SIZE;

    // タイル内に「動きうる」濡れたセルがあれば有効。動きうる = 濡れマスクが立ち(is_wet)、
    // 水・速度・浮遊顔料のどれかが残っている。
    // - 乾いたセルは velocity / advect / diffuse / transfer がすべて素通しにするので、
    //   顔料が載っていても(Fast Dry 後の沈着顔料など)計算する必要はない
    // - 蒸発で水が 0 になっても濡れマスクは残るが、水・速度・浮遊がすべて 0 のセルは全パスで
    //   不動点になる(速度は smoothstep(0, FLOW_MIN, 水) で 0、拡散と脱着の重みは水に比例、
    //   relax の発散も速度 0 なら 0)。速度は水が 0 になった次のステップで 0 に落ちるので、
    //   速度を条件に含めれば乾き切ったタイルを今と同じ結果のまま止められる
    // - 隣から水が入ってくるセル・にじみ拡張で育ちかけのマスク(0 < a ≤ 0.5)は、水の残る
    //   セルの隣にしか生じず、そのセルのタイル+tiledilate の1タイル膨張に含まれる
    // 浮遊顔料は沈着で指数的に減って厳密な 0 になりにくいので、見えない量(SUSP_EPS)は止めてよい
    var flag = 0u;
    for (var dy = 0u; dy < TILE_SIZE; dy++) {
        for (var dx = 0u; dx < TILE_SIZE; dx++) {
            let ip = vec2i(i32(x0 + dx), i32(y0 + dy));
            let cell = textureLoad(src_water, ip, 0);
            if (!is_wet(cell)) {
                continue;
            }
            let susp = textureLoad(src_susp, ip, 0);
            if (any(cell.rgb != vec3f(0.0)) || any(susp > vec4f(SUSP_EPS))) {
                flag = 1u;
            }
        }
    }

    // ブラシが触れるタイルも有効化(乾いた紙へ描き始めるとき・乾いた顔料を削るとき)。
    // 判定はタイル中心とスプラット位置のチェビシェフ距離 < ブラシ半径 + 半タイル(安全側)。
    // 筆圧で実効半径は brush_radius 以下なので、これで取りこぼさない。
    // 線画ツールのフレームは fluid_count = 0(splat は線画テクスチャにだけ描かれる)
    if (flag == 0u) {
        let cx = f32(x0) + f32(TILE_SIZE) * 0.5;
        let cy = f32(y0) + f32(TILE_SIZE) * 0.5;
        let reach = params.brush_radius + f32(TILE_SIZE);
        for (var i = 0u; i < splat_buf.fluid_count; i++) {
            let sp = splat_buf.splats[i].pos;
            if (abs(sp.x - cx) < reach && abs(sp.y - cy) < reach) {
                flag = 1u;
                break;
            }
        }
    }

    raw_active[idx] = flag;
}
