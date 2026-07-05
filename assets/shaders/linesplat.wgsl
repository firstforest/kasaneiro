// linesplat.wgsl — ラスタ線画(鉛筆/ペン)の直描き compute パス(M4.5a)。
// 流体シミュを通らず、対象の線画テクスチャ(r32float, read_write)へインク濃度を直接蓄積する。
// 対象テクスチャ(鉛筆用 / ペン用)は Rust 側の bind group で選ぶ。params.line_mode が視覚分岐:
//   0 = 鉛筆: 柔らかいエッジのグレー粒状線。紙ハイトで濃度を変調(紙目に乗る質感)。筆圧→濃さ
//   1 = ペン: 硬いエッジの濃色スムーズ線。筆圧→太さ(半径を筆圧で締める)
// params.line_eraser=1 で減算(消しゴム)。
//
// 蓄積は「目標濃度への max」: 1フレーム内で splat が密にサンプルされても濃くなりすぎず、
// 一定の線濃度に収束する(ペン先の均一な線)。より濃くしたいときは筆圧/強度で目標を上げる。
// 消しゴムだけは減算(cover×strength を引く)。
// 先頭に common.wgsl が連結される(SimParams / Splat / pressure_curve はそちら)。

@group(0) @binding(0) var line_tex: texture_storage_2d<r32float, read_write>;
@group(0) @binding(1) var<uniform> params: SimParams;
@group(0) @binding(2) var<storage, read> splat_buf: SplatBuffer;
@group(0) @binding(3) var paper_tex: texture_2d<f32>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(line_tex);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let ip = vec2i(gid.xy);
    let p = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let h = textureLoad(paper_tex, ip, 0).r;
    var ink = textureLoad(line_tex, ip).r;

    for (var i = 0u; i < splat_buf.count; i++) {
        let s = splat_buf.splats[i];
        let press = pressure_curve(s.pressure);
        let radius = max(params.brush_radius * mix(1.0, press, params.pressure_radius), 0.5);
        let dist = distance(p, s.pos);

        var deposit = 0.0;
        if (params.line_mode == 1u) {
            // ペン: 硬いエッジ(1px の遷移帯)、筆圧で太さを締める、濃度は満量
            let r = radius * mix(0.5, 1.0, press);
            let cover = 1.0 - smoothstep(r - 1.0, r, dist);
            deposit = cover * params.line_strength;
        } else {
            // 鉛筆: 柔らかいエッジ、筆圧→濃さ、紙ハイトで粒状変調(山=濃く / 谷=薄く)
            let cover = 1.0 - smoothstep(radius * 0.4, radius, dist);
            let gran = mix(1.0, h, params.line_gran);
            deposit = cover * params.line_strength * mix(1.0, press, params.pressure_pigment) * gran;
        }

        if (params.line_eraser == 1u) {
            ink -= deposit;
        } else {
            ink = max(ink, deposit);
        }
    }

    textureStore(line_tex, ip, vec4f(clamp(ink, 0.0, 1.0), 0.0, 0.0, 0.0));
}
