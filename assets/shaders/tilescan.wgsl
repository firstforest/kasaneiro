// tilescan.wgsl — アクティブタイル(M6)の第1段: タイルごとの「生」有効フラグを作る。
// 1 invocation = 1タイル。タイル内テクセルを走査して濡れ/水/顔料があるか、または
// このフレームのブラシ(splat)が近傍に触れるかを判定し、raw_active[タイル] に 0/1 を書く。
// このあと tiledilate.wgsl が1タイル分ふくらませて active を作る(にじみ前線の余裕)。
// シミュ本体より前(gpu/callback.rs)に current テクスチャを src として読む。
// active_tiles==0(最適化オフ)のときは全タイルを有効化して従来どおり全面計算に戻す。
// 先頭に common.wgsl が連結される(SimParams / SplatBuffer / TILE_SIZE / TILES_PER_SIDE)。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var src_susp: texture_2d<f32>;
@group(0) @binding(2) var src_dep: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: SimParams;
@group(0) @binding(4) var<storage, read> splat_buf: SplatBuffer;
@group(0) @binding(5) var<storage, read_write> raw_active: array<u32>;

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

    // タイル内テクセルの走査: 濡れマスク / 水量 / 浮遊・沈着顔料のどれかがあれば有効。
    // 濡れマスク(a>0.5)が立っている領域 = 湿レイヤーの生きているセルなので、
    // 乾かす/Fast Dry でマスクが 0 に戻るまでは毎フレーム計算対象になる(意図どおり)。
    var flag = 0u;
    for (var dy = 0u; dy < TILE_SIZE; dy++) {
        for (var dx = 0u; dx < TILE_SIZE; dx++) {
            let ip = vec2i(i32(x0 + dx), i32(y0 + dy));
            let w = textureLoad(src_water, ip, 0);
            let s = textureLoad(src_susp, ip, 0);
            let d = textureLoad(src_dep, ip, 0);
            if (w.a > 0.5 || w.r > 1e-4
                || dot(s, vec4f(1.0)) > 1e-4 || dot(d, vec4f(1.0)) > 1e-4) {
                flag = 1u;
            }
        }
    }

    // ブラシが触れるタイルも有効化(乾いた紙へ描き始めるときの初回スプラット用)。
    // 判定はタイル中心とスプラット位置のチェビシェフ距離 < ブラシ半径 + 半タイル(安全側)。
    // 筆圧で実効半径は brush_radius 以下なので、これで取りこぼさない。
    if (flag == 0u) {
        let cx = f32(x0) + f32(TILE_SIZE) * 0.5;
        let cy = f32(y0) + f32(TILE_SIZE) * 0.5;
        let reach = params.brush_radius + f32(TILE_SIZE);
        for (var i = 0u; i < splat_buf.count; i++) {
            let sp = splat_buf.splats[i].pos;
            if (abs(sp.x - cx) < reach && abs(sp.y - cy) < reach) {
                flag = 1u;
                break;
            }
        }
    }

    raw_active[idx] = flag;
}
