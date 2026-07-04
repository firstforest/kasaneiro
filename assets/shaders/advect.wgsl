// advect.wgsl — 移流パス(M1a: 水と速度 / M1b: 浮遊顔料)。
// セミラグランジアン法: 速度に沿って dt だけ遡った位置の値をバイリニア補間で持ってくる
// (Stam の安定流体法)。浮遊顔料は水と同じ速度場・同じバックトレースで運ぶ。
// 沈着顔料は紙に固定なので移流しない(素通し)。
// 【差し替えポイント】移流スキームを変える(風上差分など)場合はこのファイルだけ書き換える。
// 先頭に common.wgsl が連結される。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var dst_water: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var dst_susp: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var src_dep: texture_2d<f32>;
@group(0) @binding(5) var dst_dep: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_water);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_water, ip, 0);

    // 沈着顔料は移流しない(素通し。ping-pong のため必ず書く)
    textureStore(dst_dep, ip, textureLoad(src_dep, ip, 0));

    // 乾いたセルは素通し(wet-area mask)。splat が水・顔料と同時にマスクを立てるため
    // 乾いたセルの水量・浮遊顔料は常に 0 で、濡れたセルのバックトレースが乾いた領域を
    // サンプルしても水や色は湧かない(薄まるだけ)
    if (!is_wet(cell)) {
        textureStore(dst_water, ip, cell);
        textureStore(dst_susp, ip, textureLoad(src_susp, ip, 0));
        return;
    }

    // 自セルの速度で遡って(backtrace)水量・速度・浮遊顔料を取り直す
    let pos = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let back = pos - params.dt * cell.gb;
    let s = load_bilinear(src_water, back);
    let susp = load_bilinear(src_susp, back);

    // 水量・顔料は非負クランプ。a(濡れマスク)は移流させず自セルの値を保持(場に固定のため)
    textureStore(dst_water, ip, vec4f(max(s.r, 0.0), s.gb, cell.a));
    textureStore(dst_susp, ip, max(susp, vec4f(0.0)));
}
