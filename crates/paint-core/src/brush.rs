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

/// 平滑化した点として新しく採る最小の移動量(テクセル)。これより近い点は曲線の制御点にしない
/// (重なった制御点は centripetal Catmull-Rom の節点間隔を 0 にする)
const SMOOTH_MIN_STEP: f32 = 0.5;
/// 曲線補間で1区間を刻む長さ(テクセル)。この後 StrokeState が筆の間隔で引き直すので、
/// 曲線の形が折れ線に見えない程度に細かければよい
const SMOOTH_SUBDIV: f32 = 1.0;
/// ペンを離したとき、遅れているペン先を終点まで追わせる最大の歩数
const SMOOTH_CATCHUP_STEPS: usize = 64;

/// 線画の手ぶれ補正。生ポインタ点を ①遅れて追う平滑化(指数移動平均)で均し、②centripetal
/// Catmull-Rom 曲線でつないだ点列にする。出力は StrokeState::add_motion へそのまま流す。
/// 曲線は次の点が来てから前の区間を確定するので、表示は入力点1つ分遅れる。
/// 生ポインタ点だけから決まるので、同じ点列を流せば同じ線になる(線画履歴の引き直し)。
pub struct LineSmoother {
    /// 1点ごとに平滑化した位置が入力へ近づく割合(1 = 平滑化なし)
    follow: f32,
    /// 最後の生ポインタ点(ペンを離したとき、ここまで追いつく)
    last_raw: Option<([f32; 2], f32)>,
    /// 平滑化した現在の位置
    smoothed: Option<[f32; 2]>,
    /// 曲線の制御点(位置, 筆圧)。最大4点。末尾2点の間が未確定の区間
    ctrl: Vec<([f32; 2], f32)>,
}

impl LineSmoother {
    /// strength: 0 = 曲線補間だけ / 1 = 強く均す
    pub fn new(strength: f32) -> Self {
        Self {
            follow: 1.0 - 0.9 * strength.clamp(0.0, 1.0),
            last_raw: None,
            smoothed: None,
            ctrl: Vec::with_capacity(4),
        }
    }

    /// 生ポインタ点を1つ受け取り、確定した曲線上の点を out に積む
    pub fn push(&mut self, pos: [f32; 2], pressure: f32, out: &mut Vec<([f32; 2], f32)>) {
        self.last_raw = Some((pos, pressure));
        let s = match self.smoothed {
            None => pos,
            Some(prev) => lerp2(prev, pos, self.follow),
        };
        self.smoothed = Some(s);
        self.add_ctrl(s, pressure, out);
    }

    /// ペンを離したとき: 遅れているペン先を終点まで追わせ、残りの区間を確定する
    pub fn finish(&mut self, out: &mut Vec<([f32; 2], f32)>) {
        if let (Some((raw, pressure)), Some(mut s)) = (self.last_raw, self.smoothed) {
            for _ in 0..SMOOTH_CATCHUP_STEPS {
                if dist2(s, raw) < SMOOTH_MIN_STEP {
                    break;
                }
                s = lerp2(s, raw, self.follow.max(0.1));
                self.add_ctrl(s, pressure, out);
            }
            self.add_ctrl(raw, pressure, out);
        }
        let n = self.ctrl.len();
        if n >= 2 {
            let p0 = if n >= 3 {
                self.ctrl[n - 3]
            } else {
                self.ctrl[n - 2]
            };
            let (p1, p2) = (self.ctrl[n - 2], self.ctrl[n - 1]);
            emit_segment(p0, p1, p2, p2, out);
        }
        *self = Self {
            follow: self.follow,
            last_raw: None,
            smoothed: None,
            ctrl: Vec::with_capacity(4),
        };
    }

    fn add_ctrl(&mut self, pos: [f32; 2], pressure: f32, out: &mut Vec<([f32; 2], f32)>) {
        match self.ctrl.last() {
            None => out.push((pos, pressure)),
            Some(&(last, _)) if dist2(last, pos) < SMOOTH_MIN_STEP => return,
            Some(_) => {}
        }
        self.ctrl.push((pos, pressure));
        match self.ctrl.len() {
            3 => emit_segment(self.ctrl[0], self.ctrl[0], self.ctrl[1], self.ctrl[2], out),
            4 => {
                emit_segment(self.ctrl[0], self.ctrl[1], self.ctrl[2], self.ctrl[3], out);
                self.ctrl.remove(0);
            }
            _ => {}
        }
    }
}

fn lerp2(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

/// p1 → p2 の区間を centripetal Catmull-Rom で刻み、p1 を除いて p2 までを out に積む。
/// 筆圧は p1 → p2 で線形。端の区間で隣の点が無い(p0 == p1 / p3 == p2)ときは、
/// 区間を折り返した点を隣とする(節点間隔が 0 にならず、端で曲線が直線に寄る)
fn emit_segment(
    p0: ([f32; 2], f32),
    p1: ([f32; 2], f32),
    p2: ([f32; 2], f32),
    p3: ([f32; 2], f32),
    out: &mut Vec<([f32; 2], f32)>,
) {
    let (a, b) = (p1.0, p2.0);
    let len = dist2(a, b);
    let p0 = if dist2(p0.0, a) < 1e-4 {
        lerp2(a, b, -1.0)
    } else {
        p0.0
    };
    let p3 = if dist2(p3.0, b) < 1e-4 {
        lerp2(b, a, -1.0)
    } else {
        p3.0
    };
    let t1 = dist2(p0, a).sqrt();
    let t2 = t1 + len.sqrt();
    let t3 = t2 + dist2(b, p3).sqrt();
    let n = ((len / SMOOTH_SUBDIV).ceil() as usize).clamp(1, 256);
    for i in 1..=n {
        let u = i as f32 / n as f32;
        let t = t1 + (t2 - t1) * u;
        let pos = if i == n {
            b
        } else {
            let a1 = lerp2(p0, a, t / t1);
            let a2 = lerp2(a, b, (t - t1) / (t2 - t1));
            let a3 = lerp2(b, p3, (t - t2) / (t3 - t2));
            let b1 = lerp2(a1, a2, t / t2);
            let b2 = lerp2(a2, a3, (t - t1) / (t3 - t1));
            lerp2(b1, b2, (t - t1) / (t2 - t1))
        };
        out.push((pos, p1.1 + (p2.1 - p1.1) * u));
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

    fn smooth_all(strength: f32, pts: &[([f32; 2], f32)]) -> Vec<([f32; 2], f32)> {
        let mut sm = LineSmoother::new(strength);
        let mut out = Vec::new();
        for &(p, pr) in pts {
            sm.push(p, pr, &mut out);
        }
        sm.finish(&mut out);
        out
    }

    /// 補正なしでも、入力点を必ず通り、始点と終点が一致する(曲線でつなぐだけ)
    #[test]
    fn smoother_passes_through_points_without_strength() {
        let pts = [
            ([0.0, 0.0], 0.2),
            ([10.0, 0.0], 0.5),
            ([10.0, 10.0], 0.8),
            ([0.0, 10.0], 1.0),
        ];
        let out = smooth_all(0.0, &pts);
        assert_eq!(out.first().unwrap().0, [0.0, 0.0]);
        assert_eq!(out.last().unwrap().0, [0.0, 10.0]);
        for (p, _) in &pts {
            assert!(
                out.iter().any(|(q, _)| dist2(*p, *q) < 1e-3),
                "{p:?} を通らない"
            );
        }
        // 区間は 1 テクセル以下で刻まれる(折れ線に見えない)
        for w in out.windows(2) {
            assert!(dist2(w[0].0, w[1].0) <= SMOOTH_SUBDIV * 1.6, "{:?}", w);
        }
        // 筆圧は端点の値を保つ
        assert!((out.last().unwrap().1 - 1.0).abs() < 1e-6);
    }

    /// 補正を強めると、細かいジグザグ(手の震え)の振れ幅が小さくなり、終点にはちゃんと届く
    #[test]
    fn smoother_damps_jitter_and_reaches_end() {
        let pts: Vec<_> = (0..60)
            .map(|i| ([i as f32 * 2.0, if i % 2 == 0 { 1.5 } else { -1.5 }], 1.0))
            .collect();
        let swing = |out: &[([f32; 2], f32)]| {
            out.iter()
                .filter(|(p, _)| p[0] > 20.0 && p[0] < 100.0)
                .map(|(p, _)| p[1].abs())
                .fold(0.0f32, f32::max)
        };
        let raw = smooth_all(0.0, &pts);
        let smooth = smooth_all(0.8, &pts);
        assert!(
            swing(&smooth) < swing(&raw) * 0.5,
            "{} vs {}",
            swing(&smooth),
            swing(&raw)
        );
        assert_eq!(smooth.last().unwrap().0, pts.last().unwrap().0);
    }

    /// 1点だけ(タップ)でも点が1つ出る。同じ点列なら同じ結果(線画履歴の引き直し)
    #[test]
    fn smoother_tap_and_determinism() {
        assert_eq!(
            smooth_all(0.5, &[([3.0, 4.0], 0.7)]),
            vec![([3.0, 4.0], 0.7)]
        );
        let pts: Vec<_> = (0..20)
            .map(|i| ([i as f32 * 3.0, (i as f32).sin() * 5.0], 0.5))
            .collect();
        assert_eq!(smooth_all(0.5, &pts), smooth_all(0.5, &pts));
    }
}
