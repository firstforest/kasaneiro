//! 紙ハイトフィールドの生成(M1d)。Curtis 1997 の紙モデル: h ∈ [0,1] を
//! ノイズで作り、①水面勾配(velocity.wgsl)②にじみ拡張の変調(velocity.wgsl)
//! ③吸着の粒状化(transfer.wgsl)の3箇所から同じテクスチャを参照させる
//! (docs/note/01-fluid-simulation.md §6)。
//!
//! 値ノイズ(格子点ハッシュのスムーズ補間)の4成分合成:
//! - 低周波: 沈殿ムラ(大きな濃淡)
//! - 中周波: 紙目(凹凸の基調)
//! - 微細: 紙目の細かなざらつき(高周波オクターブ)
//! - 異方性: 横方向に引き伸ばした繊維ストリーク
//!
//! 実紙スキャンへの差し替え(M4)はこの生成関数をロード関数に置き換えるだけ。

/// 整数格子点 → [0,1) の決定的ハッシュ
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32)
        .wrapping_mul(0x8da6_b343)
        .wrapping_add((y as u32).wrapping_mul(0xd816_3841))
        .wrapping_add(seed.wrapping_mul(0xcb1a_b31f));
    h ^= h >> 13;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

/// 値ノイズ: 格子点のハッシュ値を smoothstep でバイリニア補間
fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    // smoothstep(C1 連続。格子の継ぎ目を消す)
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let n00 = hash(ix, iy, seed);
    let n10 = hash(ix + 1, iy, seed);
    let n01 = hash(ix, iy + 1, seed);
    let n11 = hash(ix + 1, iy + 1, seed);
    let a = n00 + (n10 - n00) * sx;
    let b = n01 + (n11 - n01) * sx;
    a + (b - a) * sy
}

/// size×size の紙ハイト(行優先、[0,1])を生成する。seed を変えると別の紙になる。
pub fn generate(size: u32, seed: u32) -> Vec<f32> {
    let mut heights = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32, y as f32);
            // 低周波 = 沈殿ムラ / 中周波 = 紙目 / 微細 = 細かなざらつき / 異方性 = 横方向の繊維ストリーク
            let low = value_noise(fx / 48.0, fy / 48.0, seed);
            let grain = value_noise(fx / 6.0, fy / 6.0, seed.wrapping_add(1));
            let micro = value_noise(fx / 2.2, fy / 2.2, seed.wrapping_add(3));
            let fiber = value_noise(fx / 24.0, fy / 3.0, seed.wrapping_add(2));
            let h = 0.34 * low + 0.26 * grain + 0.18 * micro + 0.22 * fiber;
            heights.push(h.clamp(0.0, 1.0));
        }
    }
    heights
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 値域 [0,1] と「平坦な紙になっていない」ことだけ守る(見た目は H4 の表示モードで判定)
    #[test]
    fn heights_in_range_and_varied() {
        let h = generate(64, 0);
        assert_eq!(h.len(), 64 * 64);
        let min = h.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = h.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(min >= 0.0 && max <= 1.0, "値域外: min={min} max={max}");
        assert!(max - min > 0.2, "紙が平坦すぎます: min={min} max={max}");
    }

    /// 同じ seed は同じ紙、違う seed は違う紙(決定性)
    #[test]
    fn deterministic_by_seed() {
        assert_eq!(generate(32, 7), generate(32, 7));
        assert_ne!(generate(32, 7), generate(32, 8));
    }
}
