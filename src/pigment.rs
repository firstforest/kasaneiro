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

/// GPU へ渡す latent の vec4 数: 顔料4種 × 2(c0..c3 / RGB残差)+ 紙色 × 2 + 白 × 2 + 黒 × 2。
/// 白は multiply/KM 合成で層を「白地に置いた発色」R_w を得る基準色、
/// 黒は KM 合成(M3)で層を「黒地に置いた発色」R_b を得る基準色。
/// KM は各層の反射率 R=R_b・透過率² T²=(R_w−R_b)(1−R_b) をこの2発色から導く(km.rs 参照)。
pub const LATENT_VEC4S: usize = PIGMENT_COUNT * 2 + 6;

/// 紙の色(sRGB 0..1)。display.wgsl の紙レンダリングと mixbox 混合の両方がこの値を使う
pub const PAPER_RGB: [f32; 3] = [0.96, 0.95, 0.91];

pub struct Pigment {
    pub name: &'static str,
    /// 基本色(sRGB 0..255)。mixbox 推奨の顔料色(mixbox lib.rs の PIGMENT COLORS 表)
    pub rgb: [u8; 3],
    /// 密度 ρ(M3): 沈着(吸着)の速さ。重い顔料ほど早く沈む。transfer.wgsl の deposit を倍率変調
    pub density: f32,
    /// ステイニング力 ω ∈ [0,1](M3): 剥がれにくさ。高いほど脱着・リフトが (1−ω) で抑えられ
    /// 紙に色が残る(ステイン床)。フタロ・キナクリドンは高く、アース系は低い
    pub staining: f32,
    /// 粒状感 γ ∈ [0,1](M3): 紙ハイトへの反応。高いほど凹部(谷)に沈着し凸部で剥がれる
    /// = 粒状化。粗い重い粒子(バーントシェンナ等)で高く、微細粒子(フタロ)で低い
    pub granulation: f32,
}

/// パレット(4スロット固定)。黄+青=緑の完了条件チェックは 0 と 1 で行う。
/// ρ/ω/γ は Curtis 論文の顔料表と実際の水彩の性質(handprint 等)を参照した代表値。
/// 顔料ごとの個性が「リフトで残る/紙目に溜まる」の違いとして見えることを狙う
pub const PIGMENTS: [Pigment; PIGMENT_COUNT] = [
    // 半透明・中程度のステイニング・ほぼ非粒状
    Pigment { name: "ハンザイエロー", rgb: [252, 211, 0], density: 1.0, staining: 0.5, granulation: 0.1 },
    // 微細粒子: 強ステイニング(剥がれない)・非粒状・軽い
    Pigment { name: "フタロブルー", rgb: [13, 27, 68], density: 0.8, staining: 0.9, granulation: 0.05 },
    // 透明・強ステイニング・非粒状
    Pigment { name: "キナクリドンマゼンタ", rgb: [128, 2, 46], density: 0.9, staining: 0.85, granulation: 0.05 },
    // アース系: 重く早く沈着・低ステイニング(よく剥がれる)・強粒状(紙目に溜まる)
    Pigment { name: "バーントシェンナ", rgb: [123, 72, 0], density: 1.3, staining: 0.2, granulation: 0.8 },
];

/// 全顔料+紙色の mixbox latent を uniform buffer 用に固める。
/// レイアウト(display.wgsl の `latents` と対応):
///   [2i]   = 顔料 i の (c0, c1, c2, c3)
///   [2i+1] = 顔料 i の (r残差, g残差, b残差, 0)
///   [8], [9] = 紙色の latent(同形式)
///   [10], [11] = 白の latent(層を白地に置いた発色 R_w 用。M2 multiply / M3 KM)
///   [12], [13] = 黒の latent(層を黒地に置いた発色 R_b 用。M3 KM)
pub fn latent_uniform() -> [[f32; 4]; LATENT_VEC4S] {
    let mut out = [[0.0; 4]; LATENT_VEC4S];
    for (i, pigment) in PIGMENTS.iter().enumerate() {
        let z = mixbox::rgb_to_latent(&pigment.rgb);
        out[i * 2] = [z[0], z[1], z[2], z[3]];
        out[i * 2 + 1] = [z[4], z[5], z[6], 0.0];
    }
    let mut put = |base: usize, rgb: &[f32; 3]| {
        let z = mixbox::float_rgb_to_latent(rgb);
        out[base] = [z[0], z[1], z[2], z[3]];
        out[base + 1] = [z[4], z[5], z[6], 0.0];
    };
    put(PIGMENT_COUNT * 2, &PAPER_RGB); // [8],[9] 紙
    put(PIGMENT_COUNT * 2 + 2, &[1.0, 1.0, 1.0]); // [10],[11] 白
    put(PIGMENT_COUNT * 2 + 4, &[0.0, 0.0, 0.0]); // [12],[13] 黒
    out
}

/// 顔料個性(ρ/ω/γ)を compute uniform 用に固める(M3)。
/// レイアウト(common.wgsl の binding 9 = `array<vec4f, 4>` と対応):
///   [i] = 顔料 i の (密度 ρ, ステイニング ω, 粒状感 γ, 予備 0)
/// パレットは固定なので起動時に1回書くだけ(latent_uniform と同じ扱い)。
pub fn physics_uniform() -> [[f32; 4]; PIGMENT_COUNT] {
    let mut out = [[0.0; 4]; PIGMENT_COUNT];
    for (i, p) in PIGMENTS.iter().enumerate() {
        out[i] = [p.density, p.staining, p.granulation, 0.0];
    }
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

    /// 顔料個性(M3)の値が妥当な範囲か。ステイニング/粒状感は [0,1]、密度は正。
    /// physics_uniform のレイアウト(vec4 の xyz に ρ/ω/γ)も合わせて検証する
    #[test]
    fn physics_in_range() {
        let phys = physics_uniform();
        for (i, p) in PIGMENTS.iter().enumerate() {
            assert!(p.density > 0.0, "{} の密度が非正です", p.name);
            assert!((0.0..=1.0).contains(&p.staining), "{} の ω が範囲外です", p.name);
            assert!((0.0..=1.0).contains(&p.granulation), "{} の γ が範囲外です", p.name);
            assert_eq!(phys[i], [p.density, p.staining, p.granulation, 0.0]);
        }
        // ステイニングの対比が付いていること(リフトで残る/残らないの見た目差の前提)
        assert!(
            PIGMENTS[1].staining > PIGMENTS[3].staining,
            "フタロブルーはバーントシェンナよりステイニングが強いはず"
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
