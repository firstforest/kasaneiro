//! 顔料パレット定義と mixbox 混色(M1c)。M5 でパレットをランタイム編集可能にした。
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
//!
//! M5(ランタイムパレット): 固定 const だった `PIGMENTS` を [`Palette`] の状態へ昇格。
//! UI で色・ρ/ω/γ を編集すると `pigment_latents()` / `physics_uniform()` を再計算して
//! `write_buffer`(パイプライン再構築不要)。**レイヤーごとパレット記録(M5c)**: latent は
//! 「グローバル光学(紙/白/黒)+ パレットごとの顔料ブロック」に分け、乾かした瞬間に現行
//! パレットの顔料ブロックをそのレイヤー専用スロットへ焼き込む。これで顔料を後から編集しても
//! 乾燥済みレイヤーの色は変わらない(display は保存濃度 × 記録時 latent で毎フレーム発色)。

/// 顔料スロット数(浮遊/沈着テクスチャの rgba チャンネル数と一致)
pub const PIGMENT_COUNT: usize = 4;

/// 1パレット分の顔料 latent の vec4 数: 顔料4種 × 2(c0..c3 / RGB残差)。
/// GPU の latents uniform は「グローバル光学 + パレット数 × このブロック」で構成する
pub const PIGMENT_LATENTS: usize = PIGMENT_COUNT * 2;

/// パレットに依存しないグローバル光学 latent の vec4 数: 紙色 × 2 + 白 × 2 + 黒 × 2。
/// 白は multiply/KM 合成で層を「白地に置いた発色」R_w を得る基準色、
/// 黒は KM 合成(M3)で層を「黒地に置いた発色」R_b を得る基準色。
/// KM は各層の反射率 R=R_b・透過率² T²=(R_w−R_b)(1−R_b) をこの2発色から導く(km.rs 参照)。
pub const GLOBAL_LATENTS: usize = 6;

/// 紙の色(sRGB 0..1)。display.wgsl の紙レンダリングと mixbox 混合の両方がこの値を使う
pub const PAPER_RGB: [f32; 3] = [0.96, 0.95, 0.91];

/// 顔料1種。M5 でランタイム編集可能にしたため所有型(`String` / 可変フィールド)にした。
/// M5d でパレット・ライブラリ保存のため serde 化(欠落フィールドは default で埋める)。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pigment {
    pub name: String,
    /// 基本色(sRGB 0..255)。mixbox 推奨の顔料色(mixbox lib.rs の PIGMENT COLORS 表)
    pub rgb: [u8; 3],
    /// 密度 ρ(M3): 沈着(吸着)の速さ。重い顔料ほど早く沈む。transfer.wgsl の deposit を倍率変調。
    /// ※ SimParams の `pigment_density`(表示の被覆率カーブ)とは別概念(M5a で用語を分離)
    pub density: f32,
    /// ステイニング力 ω ∈ [0,1](M3): 剥がれにくさ。高いほど脱着・リフトが (1−ω) で抑えられ
    /// 紙に色が残る(ステイン床)。フタロ・キナクリドンは高く、アース系は低い
    pub staining: f32,
    /// 粒状感 γ ∈ [0,1](M3): 紙ハイトへの反応。高いほど凹部(谷)に沈着し凸部で剥がれる
    /// = 粒状化。粗い重い粒子(バーントシェンナ等)で高く、微細粒子(フタロ)で低い
    pub granulation: f32,
}

impl Pigment {
    fn new(name: &str, rgb: [u8; 3], density: f32, staining: f32, granulation: f32) -> Self {
        Self { name: name.to_owned(), rgb, density, staining, granulation }
    }
}

/// パレット(4スロット)。M5 でランタイム編集可能な状態にした。
/// スロットは浮遊/沈着テクスチャの rgba チャンネルと 1:1(色数はライブラリ+レイヤー記録で増やす)。
/// M5d でパレット・ライブラリ(assets/palettes/*.json)とストローク記録への保存のため serde 化。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Palette {
    pub pigments: [Pigment; PIGMENT_COUNT],
}

impl Palette {
    /// 既定パレット。黄+青=緑の完了条件チェックは 0 と 1 で行う。
    /// ρ/ω/γ は Curtis 論文の顔料表と実際の水彩の性質(handprint 等)を参照した代表値。
    /// 顔料ごとの個性が「リフトで残る/紙目に溜まる」の違いとして見えることを狙う
    pub fn default_palette() -> Self {
        Self {
            pigments: [
                // 半透明・中程度のステイニング・ほぼ非粒状
                Pigment::new("ハンザイエロー", [252, 211, 0], 1.0, 0.5, 0.1),
                // 微細粒子: 強ステイニング(剥がれない)・非粒状・軽い
                Pigment::new("フタロブルー", [13, 27, 68], 0.8, 0.9, 0.05),
                // 透明・強ステイニング・非粒状
                Pigment::new("キナクリドンマゼンタ", [128, 2, 46], 0.9, 0.85, 0.05),
                // アース系: 重く早く沈着・低ステイニング(よく剥がれる)・強粒状(紙目に溜まる)
                Pigment::new("バーントシェンナ", [123, 72, 0], 1.3, 0.2, 0.8),
            ],
        }
    }

    /// このパレットの顔料 latent ブロック(8 vec4)。display.wgsl のパレットスロットに書き込む。
    /// レイアウト: [2i] = 顔料 i の (c0..c3) / [2i+1] = 顔料 i の RGB 残差(w は 0)
    pub fn pigment_latents(&self) -> [[f32; 4]; PIGMENT_LATENTS] {
        let mut out = [[0.0; 4]; PIGMENT_LATENTS];
        for (i, pigment) in self.pigments.iter().enumerate() {
            let z = mixbox::rgb_to_latent(&pigment.rgb);
            out[i * 2] = [z[0], z[1], z[2], z[3]];
            out[i * 2 + 1] = [z[4], z[5], z[6], 0.0];
        }
        out
    }

    /// 顔料個性(ρ/ω/γ)を compute uniform 用に固める(M3)。
    /// レイアウト(common.wgsl の binding 9 = `array<vec4f, 4>` と対応):
    ///   [i] = 顔料 i の (密度 ρ, ステイニング ω, 粒状感 γ, 予備 0)
    /// M5 でランタイム化。編集時に再計算して write_buffer する(全レイヤー共通=湿シミュ専用)
    pub fn physics_uniform(&self) -> [[f32; 4]; PIGMENT_COUNT] {
        let mut out = [[0.0; 4]; PIGMENT_COUNT];
        for (i, p) in self.pigments.iter().enumerate() {
            out[i] = [p.density, p.staining, p.granulation, 0.0];
        }
        out
    }
}

/// パレットに依存しないグローバル光学 latent(紙 / 白 / 黒)を固める。
/// レイアウト(display.wgsl の latents 先頭 [`GLOBAL_LATENTS`] vec4 と対応):
///   [0],[1] = 紙色 / [2],[3] = 白(R_w 用)/ [4],[5] = 黒(R_b 用、KM 合成)
pub fn global_latents() -> [[f32; 4]; GLOBAL_LATENTS] {
    let mut out = [[0.0; 4]; GLOBAL_LATENTS];
    let mut put = |base: usize, rgb: &[f32; 3]| {
        let z = mixbox::float_rgb_to_latent(rgb);
        out[base] = [z[0], z[1], z[2], z[3]];
        out[base + 1] = [z[4], z[5], z[6], 0.0];
    };
    put(0, &PAPER_RGB); // [0],[1] 紙
    put(2, &[1.0, 1.0, 1.0]); // [2],[3] 白
    put(4, &[0.0, 0.0, 0.0]); // [4],[5] 黒
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
        let pal = Palette::default_palette();
        let mix = mixbox::lerp(&pal.pigments[0].rgb, &pal.pigments[1].rgb, 0.5);
        let [r, g, b] = mix;
        assert!(g > r && g > b, "黄+青の混色が緑になっていません: rgb = {mix:?}");
    }

    /// 顔料個性(M3)の値が妥当な範囲か。ステイニング/粒状感は [0,1]、密度は正。
    /// physics_uniform のレイアウト(vec4 の xyz に ρ/ω/γ)も合わせて検証する
    #[test]
    fn physics_in_range() {
        let pal = Palette::default_palette();
        let phys = pal.physics_uniform();
        for (i, p) in pal.pigments.iter().enumerate() {
            assert!(p.density > 0.0, "{} の密度が非正です", p.name);
            assert!((0.0..=1.0).contains(&p.staining), "{} の ω が範囲外です", p.name);
            assert!((0.0..=1.0).contains(&p.granulation), "{} の γ が範囲外です", p.name);
            assert_eq!(phys[i], [p.density, p.staining, p.granulation, 0.0]);
        }
        // ステイニングの対比が付いていること(リフトで残る/残らないの見た目差の前提)
        assert!(
            pal.pigments[1].staining > pal.pigments[3].staining,
            "フタロブルーはバーントシェンナよりステイニングが強いはず"
        );
    }

    /// 顔料 latent ブロックのレイアウト検証: latent → rgb の復元が基本色と(量子化誤差内で)一致
    #[test]
    fn latent_roundtrip() {
        let pal = Palette::default_palette();
        let latents = pal.pigment_latents();
        for (i, pigment) in pal.pigments.iter().enumerate() {
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

    /// グローバル光学 latent(紙)の復元が PAPER_RGB と一致すること(レイアウトの回帰チェック)
    #[test]
    fn global_paper_roundtrip() {
        let g = global_latents();
        let z = [g[0][0], g[0][1], g[0][2], g[0][3], g[1][0], g[1][1], g[1][2]];
        let rgb = mixbox::latent_to_rgb(&z);
        for c in 0..3 {
            let expect = (PAPER_RGB[c] * 255.0).round() as i32;
            assert!(
                (rgb[c] as i32 - expect).abs() <= 1,
                "紙色 latent の復元がずれています: {rgb:?}"
            );
        }
    }
}
