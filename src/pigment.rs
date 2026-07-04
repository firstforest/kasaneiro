//! 顔料パレット定義と mixbox 混色(M1c)。
//!
//! **mixbox クレートの呼び出しはこのファイルに隔離する**(plan.md §4: CC BY-NC のため
//! 商用化時に自作スペクトラル WGM へ差し替える。その影響範囲をここだけに閉じる)。
//!
//! 混色の仕組み(LUT の WGSL 移植を回避する分担):
//! - CPU(ここ): 各顔料の基本色 sRGB → mixbox latent(7 floats)。LUT 参照はこの1回だけ。
//!   結果は uniform buffer で GPU へ渡す(gpu/mod.rs)
//! - GPU(display.wgsl): 画素ごとに 4 顔料の濃度で latent を線形混合し、
//!   latent → RGB 多項式(mixbox eval_polynomial の WGSL 移植)で発色する
//!
//! 顔料スロットはテクスチャの rgba 4 チャンネルと 1:1 対応(sim/mod.rs のコメント参照)。

/// 顔料スロット数(浮遊/沈着テクスチャの rgba チャンネル数と一致)
pub const PIGMENT_COUNT: usize = 4;

/// GPU へ渡す latent の vec4 数: 顔料4種 × 2(c0..c3 / RGB残差)+ 紙色 × 2
pub const LATENT_VEC4S: usize = PIGMENT_COUNT * 2 + 2;

/// 紙の色(sRGB 0..1)。display.wgsl の紙レンダリングと mixbox 混合の両方がこの値を使う
pub const PAPER_RGB: [f32; 3] = [0.96, 0.95, 0.91];

pub struct Pigment {
    pub name: &'static str,
    /// 基本色(sRGB 0..255)。mixbox 推奨の顔料色(mixbox lib.rs の PIGMENT COLORS 表)
    pub rgb: [u8; 3],
}

/// パレット(4スロット固定)。黄+青=緑の完了条件チェックは 0 と 1 で行う
pub const PIGMENTS: [Pigment; PIGMENT_COUNT] = [
    Pigment { name: "ハンザイエロー", rgb: [252, 211, 0] },
    Pigment { name: "フタロブルー", rgb: [13, 27, 68] },
    Pigment { name: "キナクリドンマゼンタ", rgb: [128, 2, 46] },
    Pigment { name: "バーントシェンナ", rgb: [123, 72, 0] },
];

/// 全顔料+紙色の mixbox latent を uniform buffer 用に固める。
/// レイアウト(display.wgsl の `latents` と対応):
///   [2i]   = 顔料 i の (c0, c1, c2, c3)
///   [2i+1] = 顔料 i の (r残差, g残差, b残差, 0)
///   [8], [9] = 紙色の latent(同形式)
pub fn latent_uniform() -> [[f32; 4]; LATENT_VEC4S] {
    let mut out = [[0.0; 4]; LATENT_VEC4S];
    for (i, pigment) in PIGMENTS.iter().enumerate() {
        let z = mixbox::rgb_to_latent(&pigment.rgb);
        out[i * 2] = [z[0], z[1], z[2], z[3]];
        out[i * 2 + 1] = [z[4], z[5], z[6], 0.0];
    }
    let z = mixbox::float_rgb_to_latent(&PAPER_RGB);
    out[PIGMENT_COUNT * 2] = [z[0], z[1], z[2], z[3]];
    out[PIGMENT_COUNT * 2 + 1] = [z[4], z[5], z[6], 0.0];
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M1c の完了条件の CPU 版: 黄+青の latent 混合が緑になること。
    /// display.wgsl の多項式移植が正しいかは目視(GPU とCPU で同じ latent を使うため、
    /// ここが緑なら GPU 側が濁る原因は移植ミスに絞れる)
    #[test]
    fn yellow_plus_blue_is_green() {
        let mix = mixbox::lerp(&PIGMENTS[0].rgb, &PIGMENTS[1].rgb, 0.5);
        let [r, g, b] = mix;
        assert!(
            g > r && g > b,
            "黄+青の混色が緑になっていません: rgb = {mix:?}"
        );
    }

    /// latent uniform のレイアウト検証: latent → rgb の復元が基本色と(量子化誤差内で)一致
    #[test]
    fn latent_roundtrip() {
        let latents = latent_uniform();
        for (i, pigment) in PIGMENTS.iter().enumerate() {
            let z = [
                latents[i * 2][0],
                latents[i * 2][1],
                latents[i * 2][2],
                latents[i * 2][3],
                latents[i * 2 + 1][0],
                latents[i * 2 + 1][1],
                latents[i * 2 + 1][2],
            ];
            let rgb = mixbox::latent_to_rgb(&z);
            for c in 0..3 {
                assert!(
                    (rgb[c] as i32 - pigment.rgb[c] as i32).abs() <= 1,
                    "{} の latent 復元がずれています: {:?} != {:?}",
                    pigment.name,
                    rgb,
                    pigment.rgb
                );
            }
        }
    }
}
