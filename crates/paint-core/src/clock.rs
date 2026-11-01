//! シミュレーションの固定刻み時計。
//!
//! 蒸発・拡散などはステップ数で進むので、画面のフレームごとに1ステップ回すと
//! リフレッシュレートが高いほど速く乾く。経過時間を積み立てて 1/[`TICK_HZ`] 秒ごとに
//! 1刻みを出し、シミュのステップ数・記録再生の時刻をどちらも刻み単位にそろえる。
//! headless(e2e)は1フレーム=1刻みで回すので、実機の 60Hz と同じ進み方になる。

/// 1秒あたりの刻み数(e2e と既存のストローク記録が前提にしている 60fps に合わせる)
pub const TICK_HZ: f32 = 60.0;

/// 1フレームで出す刻みの上限。描画が重くて dt が伸びたとき、追いつこうとして
/// さらに重くなる悪循環を避けるため、超えた分は捨てて実時間より遅れることを許す
pub const MAX_TICKS_PER_FRAME: u32 = 4;

#[derive(Clone, Copy, Debug, Default)]
pub struct SimClock {
    /// 刻みに満たない積み残し(秒)
    carry: f32,
}

impl SimClock {
    /// フレームの経過時間 dt(秒)を積み、このフレームで進める刻み数を返す
    pub fn advance(&mut self, dt: f32) -> u32 {
        let tick = 1.0 / TICK_HZ;
        self.carry += dt.max(0.0);
        // 丸め誤差で 0.9999 刻みが 0 に落ちて 60Hz の刻みが 0,2,0,2 と揺れないよう少しだけ甘く数える
        let ticks = (self.carry / tick + 1e-3).floor();
        if ticks >= MAX_TICKS_PER_FRAME as f32 {
            self.carry = 0.0;
            return MAX_TICKS_PER_FRAME;
        }
        self.carry -= ticks * tick;
        ticks as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(hz: f32, seconds: f32) -> u32 {
        let mut clock = SimClock::default();
        let frames = (hz * seconds).round() as u32;
        (0..frames).map(|_| clock.advance(1.0 / hz)).sum()
    }

    /// リフレッシュレートが違っても、同じ実時間で進む刻み数はそろう
    #[test]
    fn ticks_follow_wall_time_across_refresh_rates() {
        for hz in [30.0, 60.0, 75.0, 120.0, 144.0, 240.0] {
            let ticks = run(hz, 10.0);
            assert!((ticks as i32 - 600).abs() <= 1, "{hz}Hz: {ticks} 刻み");
        }
    }

    /// 60Hz では毎フレームちょうど1刻み(e2e・既存の記録と同じ進み方)
    #[test]
    fn one_tick_per_frame_at_60hz() {
        let mut clock = SimClock::default();
        for _ in 0..1000 {
            assert_eq!(clock.advance(1.0 / 60.0), 1);
        }
    }

    /// 長く止まったフレームは上限で打ち切り、積み残しも捨てる
    #[test]
    fn long_frame_is_capped() {
        let mut clock = SimClock::default();
        assert_eq!(clock.advance(1.0), MAX_TICKS_PER_FRAME);
        assert_eq!(clock.advance(0.0), 0);
    }
}
