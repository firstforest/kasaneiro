// bake.wgsl — 「乾かす」= 定着パス(M2)。手動ボタンで 1 回だけ走る(毎フレームではない)。
// 湿レイヤーの顔料(浮遊+沈着)を乾燥レイヤー(binding 9 = texture array の1スライス、
// rgba = 4顔料濃度)へ焼き込み、湿レイヤー(水・速度・濡れマスク・顔料)を全ゼロに戻す。
// 焼き込み時に掛かる3つの効果(いずれも試行錯誤対象 = plan.md M2):
//   dry shift   — 乾くと薄くなる(dry_shift < 1)
//   粒状感ゲート — 紙の凹部で濃く/凸部で薄く定着(dry_gran)
//   エッジダークニング — 濡れ領域の縁バンドで濃度を増す(dry_edge。Curtis のエッジ
//     ダークニングは乾燥時の現象なのでここで掛ける。縁バンド = 濡れマスクのボックス
//     ぼかしが 1 を割る帯、幅は edge_radius。M1d の FlowOutward とは独立)
// binding は共通レイアウトの 0..6, 8 + 専用の 9(splats の 7 は使わない)。
// 先頭に common.wgsl が連結される。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var dst_water: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var dst_susp: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var src_dep: texture_2d<f32>;
@group(0) @binding(5) var dst_dep: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(8) var paper_tex: texture_2d<f32>;
@group(0) @binding(9) var dst_layer: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_water);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_water, ip, 0);
    let susp = textureLoad(src_susp, ip, 0);
    let dep = textureLoad(src_dep, ip, 0);

    // 濡れ領域の縁バンド: マスクのボックスぼかし M' に対し max(M − M', 0)。
    // 内部は M'≈1 で 0、縁ほど大きい。乾いたセル(M=0)は常に 0
    var band = 0.0;
    if (params.dry_edge > 0.0 && is_wet(cell)) {
        let r = i32(clamp(params.edge_radius, 1u, 8u));
        var sum = 0.0;
        var n = 0.0;
        for (var dy = -r; dy <= r; dy++) {
            for (var dx = -r; dx <= r; dx++) {
                sum += load_clamped(src_water, ip + vec2i(dx, dy)).a;
                n += 1.0;
            }
        }
        band = max(cell.a - sum / n, 0.0);
    }

    // 粒状感ゲート: 凹部(h=0)で ×(1+gran)、凸部(h=1)で ×(1−gran)
    let h = textureLoad(paper_tex, ip, 0).r;
    let gran = max(1.0 + params.dry_gran * (1.0 - 2.0 * h), 0.0);

    let conc = max(susp + dep, vec4f(0.0));
    let baked = conc * params.dry_shift * gran * (1.0 + params.dry_edge * band);
    textureStore(dst_layer, ip, baked);

    // 湿レイヤーを解放(水・速度・濡れマスク・顔料すべてゼロ = 乾いた紙に戻る)
    textureStore(dst_water, ip, vec4f(0.0));
    textureStore(dst_susp, ip, vec4f(0.0));
    textureStore(dst_dep, ip, vec4f(0.0));
}
