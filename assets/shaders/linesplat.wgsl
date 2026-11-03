// linesplat.wgsl — ラスタ線画(鉛筆/ペン/ハイライト)の直描き compute パス。
// 流体シミュを通らず、対象の線画テクスチャ(r32float, read_write)へインク濃度を直接蓄積する。
// 対象テクスチャ(鉛筆 / ペン / ハイライト)は Rust 側の bind group で選ぶ。params.line_mode が視覚分岐:
//   0 = 鉛筆: 柔らかいエッジのグレー粒状線。紙ハイトで濃度を変調(紙目に乗る質感)。筆圧→濃さ
//   1 = ペン: 1px の縁でアンチエイリアスした濃色スムーズ線。筆圧→太さ
//   2 = ハイライト: 硬めエッジの不透明白。筆圧→不透明度。合成の最上段に白として重ねる
// params.line_eraser=1 で減算(消しゴム)。
//
// 蓄積は「目標濃度への max」: 1フレーム内で splat が密にサンプルされても濃くなりすぎず、
// 一定の線濃度に収束する(ペン先の均一な線)。ペンは描き始めに複製した下地(pen_base)に
// 1本ぶんの濃さを重ねた値を目標にするので、1本の中は均一で、別のストロークと重なった所だけ濃くなる。
// 消しゴムだけは減算(cover×strength を引く)。
// 先頭に common.wgsl が連結される(SimParams / Splat / pressure_curve はそちら)。

// ペンの線を全量の濃さで描く最小の半径(px)。これより細い線は半径をここで止め、細さの分だけ
// 薄くする(1px を下回る線がピクセル中心を外れて点線に欠けないように)
const PEN_MIN_RADIUS: f32 = 0.7;

@group(0) @binding(0) var line_tex: texture_storage_2d<r32float, read_write>;
@group(0) @binding(1) var<uniform> params: SimParams;
@group(0) @binding(2) var<storage, read> splat_buf: SplatBuffer;
@group(0) @binding(3) var paper_tex: texture_2d<f32>;
@group(0) @binding(4) var pen_base: texture_2d<f32>;

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
    let under = textureLoad(pen_base, ip, 0).r;

    // 鉛筆/ペン/ハイライトで独立した半径を使い(brush_radius = 水ブラシとは切り離す)、
    // 消しゴムはレイヤーごとの消しゴムの半径を使う(SimParams::line_radius と同じ対応)
    let eraser = params.line_eraser == 1u;
    var base_radius = select(params.pencil_radius, params.pencil_eraser_radius, eraser);
    if (params.line_mode == 1u) {
        base_radius = select(params.pen_radius, params.pen_eraser_radius, eraser);
    } else if (params.line_mode == 2u) {
        base_radius = select(params.highlight_radius, params.highlight_eraser_radius, eraser);
    }

    for (var i = 0u; i < splat_buf.count; i++) {
        let s = splat_buf.splats[i];
        let press = pressure_curve(s.pressure);
        let radius = max(base_radius * mix(1.0, press, params.pressure_radius), 0.5);
        let dist = distance(p, s.pos);

        var deposit = 0.0;
        if (params.line_mode == 1u) {
            // ペン: 縁を中心にした 1px の遷移帯、筆圧で太さを締める。細すぎる線は太さの代わりに濃さを落とす
            let r_raw = base_radius * mix(1.0, press, params.pressure_radius);
            let r = max(r_raw, PEN_MIN_RADIUS);
            let cover = clamp(r + 0.5 - dist, 0.0, 1.0);
            deposit = cover * params.pen_strength * clamp(r_raw / PEN_MIN_RADIUS, 0.0, 1.0);
        } else if (params.line_mode == 2u) {
            // ハイライト: 硬めエッジの不透明白、筆圧→不透明度
            let cover = 1.0 - smoothstep(radius - 1.0, radius, dist);
            deposit = cover * params.highlight_strength * mix(1.0, press, params.pressure_pigment);
        } else {
            // 鉛筆: 柔らかいエッジ、筆圧→濃さ、紙ハイトで粒状変調(山=濃く / 谷=薄く)
            let cover = 1.0 - smoothstep(radius * 0.4, radius, dist);
            let gran = mix(1.0, h, params.pencil_gran);
            deposit = cover * params.pencil_strength * mix(1.0, press, params.pressure_pigment) * gran;
        }

        if (params.line_eraser == 1u) {
            ink -= deposit;
        } else if (params.line_mode == 1u) {
            ink = max(ink, under + deposit * (1.0 - under));
        } else {
            ink = max(ink, deposit);
        }
    }

    textureStore(line_tex, ip, vec4f(clamp(ink, 0.0, 1.0), 0.0, 0.0, 0.0));
}
