//! ストローク → splat 列への変換。
//! ポインタイベントは飛び飛びに来るので、前回位置から等間隔に補間して隙間のない線にする。

use crate::sim::Splat;

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
    /// spacing_at は筆圧 → サンプル間隔(テクセル)。ブラシ半径の 1/4 程度が目安。
    /// 間隔は区間内の各地点の筆圧から決めるので、筆圧が大きく変わる区間(勢いよく払った
    /// 最後の1区間など)でも splat の密度が半径に見合ったままになる。端点の筆圧1つで決めると、
    /// 抜きの区間の太い部分に細い筆用の間隔で splat が詰まり、筆の中身を使い切って払いの先が薄く抜ける。
    /// 各 splat にはストローク方向の単位ベクトルを持たせる(水の初速の向きに使う)。
    pub fn add_motion(
        &mut self,
        pos: [f32; 2],
        pressure: f32,
        spacing_at: impl Fn(f32) -> f32,
        out: &mut Vec<Splat>,
    ) {
        let spacing = |p: f32| spacing_at(p).max(0.5);
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
                if dist < spacing(prev_pressure).min(spacing(pressure)) {
                    // 間隔に満たない移動は溜めておき、次の点とまとめて補間する
                    return;
                }
                let dir = [dx / dist, dy / dist];
                // 筆圧も位置と同様に線形補間(M1.5: 筆入れ・筆抜きを滑らかに)
                let pressure_at = |t: f32| prev_pressure + (pressure - prev_pressure) * t;
                // その地点の筆圧の間隔で歩き、終点を越えた分は全体を縮めて最後の点を終点に合わせる
                let mut ts = Vec::new();
                let mut s = 0.0;
                while s < dist {
                    s += spacing(pressure_at(s / dist));
                    ts.push(s);
                }
                for t in ts {
                    let t = t / s;
                    out.push(Splat::new(
                        [prev[0] + dx * t, prev[1] + dy * t],
                        dir,
                        pressure_at(t),
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
        stroke.add_motion([0.0, 0.0], 0.0, |_| 1.0, &mut out);
        stroke.add_motion([4.0, 0.0], 1.0, |_| 1.0, &mut out);
        assert_eq!(out.len(), 5); // 始点 + 補間4点
        assert_eq!(out[0].pressure, 0.0);
        assert_eq!(out.last().unwrap().pressure, 1.0);
        for pair in out.windows(2) {
            assert!(pair[0].pressure <= pair[1].pressure, "単調増加でない");
        }
    }

    /// 1区間で筆圧が大きく下がっても(勢いよく払った最後の1区間)、太い所に細い筆用の
    /// 間隔が使われて splat が詰まらないこと。間隔は筆圧に比例する半径の 1/4
    #[test]
    fn spacing_follows_pressure_within_segment() {
        let spacing_at = |p: f32| (40.0 * p).max(4.0) * 0.25;
        let mut stroke = StrokeState::default();
        let mut out = Vec::new();
        stroke.add_motion([0.0, 0.0], 1.0, spacing_at, &mut out);
        stroke.add_motion([100.0, 0.0], 0.0, spacing_at, &mut out);
        let last = out.last().unwrap();
        assert!((last.pos[0] - 100.0).abs() < 1e-3);
        assert!(last.pressure.abs() < 1e-6);
        // 隣り合う splat の間隔は、その地点の筆圧の間隔を超えない(隙間を作らない)
        for pair in out.windows(2) {
            let gap = pair[1].pos[0] - pair[0].pos[0];
            assert!(
                gap > 0.0 && gap <= spacing_at(pair[0].pressure) + 1e-3,
                "gap {gap}"
            );
        }
        // 太い前半は粗く、細い後半は細かく並ぶ
        let thick = out.iter().filter(|s| s.pressure > 0.5).count();
        let thin = out.iter().filter(|s| s.pressure <= 0.5).count();
        assert!(thick < thin, "{thick} vs {thin}");
        // 終点の筆圧(細い筆)の間隔で区間全体を埋めた場合(100 本)より十分少ない
        assert!(out.len() < 60, "{} splats", out.len());
    }
}
