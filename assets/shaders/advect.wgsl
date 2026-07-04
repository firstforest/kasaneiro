// advect.wgsl — 移流パス(M1a)。水量と速度をセミラグランジアン法で運ぶ。
// 速度に沿って dt だけ遡った位置の値をバイリニア補間で持ってくる(Stam の安定流体法)。
// 【差し替えポイント】移流スキームを変える(風上差分など)場合はこのファイルだけ書き換える。
// 先頭に common.wgsl が連結される。

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> params: SimParams;
@group(0) @binding(3) var<storage, read> splat_buf: SplatBuffer;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_tex);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_tex, ip, 0);

    // 自セルの速度で遡って(backtrace)水量と速度を取り直す
    let pos = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let back = pos - params.dt * cell.gb;
    let s = load_bilinear(src_tex, back);

    // 水量は非負クランプ。a(予備)は移流させず自セルの値を保持(紙ハイト等は場に固定のため)
    textureStore(dst_tex, ip, vec4f(max(s.r, 0.0), s.gb, cell.a));
}
