//! ストローク → splat 列への変換。
//! ポインタイベントは飛び飛びに来るので、前回位置から等間隔に補間して隙間のない線にする。

use crate::sim::{MAX_SPLATS, Splat};

#[derive(Default)]
pub struct StrokeState {
    /// 直前のサンプル(位置, 筆圧)。筆圧も補間する(M1.5)
    last: Option<([f32; 2], f32)>,
}

impl StrokeState {
    pub fn begin(&mut self) {
        self.last = None;
    }

    pub fn end(&mut self) {
        self.last = None;
    }

    /// ポインタ位置(テクセル座標)を受け取り、補間した splat 列を out に積む。
    /// spacing はサンプル間隔(テクセル)。ブラシ半径の 1/4 程度が目安。
    /// 各 splat にはストローク方向の単位ベクトルを持たせる(水の初速の向きに使う)。
    pub fn add_motion(&mut self, pos: [f32; 2], pressure: f32, spacing: f32, out: &mut Vec<Splat>) {
        let spacing = spacing.max(0.5);
        match self.last {
            None => {
                // 始点は方向が定まらないので初速なし
                out.push(Splat::new(pos, [0.0, 0.0], pressure));
                self.last = Some((pos, pressure));
            }
            Some((prev, prev_pressure)) => {
                let dx = pos[0] - prev[0];
                let dy = pos[1] - prev[1];
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < spacing {
                    // 間隔に満たない移動は溜めておき、次の点とまとめて補間する
                    return;
                }
                let dir = [dx / dist, dy / dist];
                let steps = (dist / spacing).ceil() as usize;
                for i in 1..=steps {
                    if out.len() >= MAX_SPLATS {
                        break;
                    }
                    let t = i as f32 / steps as f32;
                    out.push(Splat::new(
                        [prev[0] + dx * t, prev[1] + dy * t],
                        dir,
                        // 筆圧も位置と同様に線形補間(M1.5: 筆入れ・筆抜きを滑らかに)
                        prev_pressure + (pressure - prev_pressure) * t,
                    ));
                }
                self.last = Some((pos, pressure));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 筆圧が始点→終点で線形に補間されること(M1.5)
    #[test]
    fn pressure_interpolates() {
        let mut stroke = StrokeState::default();
        let mut out = Vec::new();
        stroke.add_motion([0.0, 0.0], 0.0, 1.0, &mut out);
        stroke.add_motion([4.0, 0.0], 1.0, 1.0, &mut out);
        assert_eq!(out.len(), 5); // 始点 + 補間4点
        assert_eq!(out[0].pressure, 0.0);
        assert_eq!(out.last().unwrap().pressure, 1.0);
        for pair in out.windows(2) {
            assert!(pair[0].pressure <= pair[1].pressure, "単調増加でない");
        }
    }
}
