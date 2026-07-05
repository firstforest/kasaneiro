//! Kubelka-Munk 層合成の純関数(M3)。plan.md §3 / note 03 §3 の式の CPU 参照実装で、
//! `cargo test` の対象(流体シェーダーはテストせずデバッグ表示で診断する方針の例外 = §1)。
//!
//! **設計判断(status.md M3 参照)**: 層「内」の混色は M1c の mixbox のまま維持する
//! (RGB 3チャンネルの K,S で顔料を混ぜると黄+青が濁り、M1c で mixbox に移した意味が消える)。
//! KM は層「間」の光学スタック(グレーズの重なり)だけに使う。各層の反射率 R・透過率 T は、
//! **その層を白地・黒地に置いた mixbox 発色 R_w, R_b から導く**。
//!
//! なぜ簡約できるか: 単層(R,T)を反射率 Rbg の背景に置いたときの見かけの反射率は
//! `Rbg=1`(白)で `R_w = R + T²/(1−R)`、`Rbg=0`(黒)で `R_b = R` になる(合成式に代入)。
//! これを R,T について解くと **`R = R_b`, `T² = (R_w − R_b)(1 − R_b)`** と閉じた形になり、
//! sinh/cosh を経由しない(§3 の「厚みが大きいとオーバーフロー」対策が不要)。
//! この簡約が Curtis 一般式(K,S → R,T)と一致することを下のテストで担保してから
//! display.wgsl へ移植する。
//!
//! 一般式(composite / composite_over / ks_from_rw_rb / rt_from_ks)は簡約の検証と
//! 将来の参照のための CPU 実装で、製品コードからは呼ばれない(実合成は display.wgsl)。
#![allow(dead_code)]

/// coth⁻¹(y) = ½·ln((y+1)/(y−1))(§3 の数値上の注意)。|y| > 1 で定義。
pub fn acoth(y: f32) -> f32 {
    0.5 * ((y + 1.0) / (y - 1.0)).ln()
}

/// ユーザー指定色(白背景 R_w / 黒背景 R_b)から吸収係数 K・散乱係数 S を逆算(§5.1)。
/// 単位厚み(x=1)を前提とする。各チャンネル独立に呼ぶ。
/// 0 < R_b < R_w < 1 を要求する(ゼロ除算・NaN 回避)ため、呼び出し側でクランプ済みとする。
pub fn ks_from_rw_rb(rw: f32, rb: f32) -> (f32, f32) {
    let a = 0.5 * (rw + (rb - rw + 1.0) / rb);
    let b = (a * a - 1.0).max(0.0).sqrt();
    let s = (1.0 / b) * acoth((b * b - (a - rw) * (a - 1.0)) / (b * (1.0 - rw)));
    let k = s * (a - 1.0);
    (k, s)
}

/// 吸収 K・散乱 S・厚み x の単層の反射率 R・透過率 T(§3)。
/// sinh/cosh は厚みが大きいとオーバーフローするので引数をクランプする(§3 の注意)。
pub fn rt_from_ks(k: f32, s: f32, x: f32) -> (f32, f32) {
    let s = s.max(1e-4); // 純吸収体(S→0)は a=1+K/S が発散するので下限でクランプ
    let a = 1.0 + k / s;
    let b = (a * a - 1.0).max(0.0).sqrt();
    let arg = (b * s * x).min(20.0); // e^20 まで。これ以上は R が飽和するだけ
    let sh = arg.sinh();
    let ch = arg.cosh();
    let c = a * sh + b * ch;
    (sh / c, b / c)
}

/// 2層の合成(Kubelka の合成式、§3)。上層 (r1,t1)・下層 (r2,t2) → 合成 (R,T)。
pub fn composite(r1: f32, t1: f32, r2: f32, t2: f32) -> (f32, f32) {
    let denom = 1.0 - r1 * r2;
    let r = r1 + t1 * t1 * r2 / denom;
    let t = t1 * t2 / denom;
    (r, t)
}

/// 上に見る用の反射率だけを畳み込む簡約版。下地(それより下すべての反射率)`r_below` の上に
/// 反射率 `r_top`・透過率² `t2_top` の層を重ねたときの、上から見た反射率。
/// 最下層が紙(不透明)で常に上から見るため、合成後の透過率は不要(§3 の合成式の R 部だけ)。
pub fn composite_over(r_top: f32, t2_top: f32, r_below: f32) -> f32 {
    let denom = (1.0 - r_top * r_below).max(1e-4);
    r_top + t2_top * r_below / denom
}

/// 層を白地・黒地に置いた発色 (R_w, R_b) から、その層の反射率 R と透過率² T² を導く簡約
/// (このファイル冒頭の導出)。display.wgsl の KM 合成が使う形。R_w ≥ R_b を前提にクランプ。
pub fn layer_r_t2_from_backgrounds(rw: f32, rb: f32) -> (f32, f32) {
    let r = rb;
    let t2 = (rw - rb).max(0.0) * (1.0 - rb);
    (r, t2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 簡約(白/黒地の発色 → R,T²)が Curtis 一般式(K,S → R,T)と一致すること。
    /// 一般式で作った層の白地/黒地での見かけ反射率を簡約に食わせ、元の (R, T²) を復元する。
    /// これが display.wgsl で sinh/cosh を使わず合成してよい根拠。
    #[test]
    fn reduction_matches_general() {
        for &k in &[0.02f32, 0.1, 0.5, 1.5] {
            for &s in &[0.05f32, 0.3, 1.0, 2.0] {
                let (r, t) = rt_from_ks(k, s, 1.0);
                // 一般式の層を白地(反射率1)・黒地(0)に置いた見かけの反射率
                let rw = composite_over(r, t * t, 1.0);
                let rb = composite_over(r, t * t, 0.0);
                let (r2, t2) = layer_r_t2_from_backgrounds(rw, rb);
                assert!((r2 - r).abs() < 1e-4, "R 不一致: {r2} != {r} (K={k} S={s})");
                assert!(
                    (t2 - t * t).abs() < 1e-4,
                    "T² 不一致: {t2} != {} (K={k} S={s})",
                    t * t
                );
            }
        }
    }

    /// ks_from_rw_rb → rt_from_ks(x=1) の往復: 指定した R_w, R_b を再現すること(§5.1 の逆算の検証)。
    #[test]
    fn ks_roundtrip() {
        for &(rw, rb) in &[(0.9f32, 0.1f32), (0.7, 0.3), (0.5, 0.05), (0.95, 0.6)] {
            let (k, s) = ks_from_rw_rb(rw, rb);
            let (r, t) = rt_from_ks(k, s, 1.0);
            let rw2 = composite_over(r, t * t, 1.0);
            let rb2 = composite_over(r, t * t, 0.0);
            assert!((rw2 - rw).abs() < 2e-3, "R_w 復元ずれ: {rw2} != {rw}");
            assert!((rb2 - rb).abs() < 2e-3, "R_b 復元ずれ: {rb2} != {rb}");
        }
    }

    /// 透明な層(白地=1・黒地=0)は下地をそのまま通す(R≈0, T²≈1)。
    #[test]
    fn transparent_layer_passes_through() {
        let (r, t2) = layer_r_t2_from_backgrounds(1.0, 0.0);
        assert!(r.abs() < 1e-6);
        assert!((t2 - 1.0).abs() < 1e-6);
        // どんな下地でも変えない
        for &below in &[0.0f32, 0.3, 0.9] {
            assert!((composite_over(r, t2, below) - below).abs() < 1e-6);
        }
    }

    /// 不透明な層(白地=黒地=同色 → T²=0)は下地を完全に隠す。
    #[test]
    fn opaque_layer_hides_below() {
        let (r, t2) = layer_r_t2_from_backgrounds(0.3, 0.3);
        assert!((r - 0.3).abs() < 1e-6);
        assert!(t2.abs() < 1e-6);
        for &below in &[0.0f32, 0.5, 1.0] {
            assert!((composite_over(r, t2, below) - 0.3).abs() < 1e-6);
        }
    }

    /// グレーズ(半透明層)は下地が明るいほど明るく見える(下の色が透ける = 光学混色の芯)。
    #[test]
    fn glaze_shows_background() {
        let (r, t2) = layer_r_t2_from_backgrounds(0.6, 0.2); // 半透明
        let on_white = composite_over(r, t2, 0.95);
        let on_black = composite_over(r, t2, 0.02);
        assert!(
            on_white > on_black,
            "下地が透けていない: {on_white} <= {on_black}"
        );
        // 黒地では層の固有反射率(R_b)にほぼ一致
        assert!((on_black - 0.2).abs() < 0.02);
    }
}
