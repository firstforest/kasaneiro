//! 筆の中身(含んでいる水と顔料)。
//!
//! 単位は「標準の1回分」: 水 1 = 満タン、顔料 1 = 満タンの水に色1回分を溶かした量。
//! 濃さ = 顔料の合計 ÷ 水。
//!
//! 中身の正典は GPU の筆バッファ([`BrushState`])。splat.wgsl がそこから `brush_water` /
//! `brush_pigment` を掛けて注ぎ、紙から吸い込んだ分を集計して、brush.wgsl がフレームの最後に
//! なぞった減り・置いたままの含み・吸い込みを中身へ反映する。CPU は [`BrushFrame`] で
//! 「中身を置き換える」「どれだけなぞった・置いたままだった」を毎フレーム渡し、
//! 読み戻した中身を表示と次の操作の起点に使う。
//!
//! 中身を変えるのはツールバーのスウォッチだけ:
//! - 色: 押した瞬間に1回分、押している間も少しずつ足す(濃くなる)
//! - 水: 押した瞬間に顔料を半分、押している間も水と入れ替わって抜けていく(薄くなる・洗える)
//! - 洗う: 水も顔料も 0(空の筆)。続けて水に浸せば水だけの筆になる
//!
//! 描くと中身が減る(`brush_hold` px なぞると半分)。水と顔料は同じ割合で減るので濃さは変わらない。
//! 減った筆で濡れた色をなでると、紙のほうが濡れている分だけ水と浮いている色を吸い込む
//! (`brush_absorb`)ので、筆の色が下の色に寄っていく。

use crate::sim::Splat;
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// 色スウォッチを押した瞬間に足す顔料(標準の1回分)
pub const TAKE_DOSE: f32 = 1.0;
/// 色スウォッチを押し続けている間に足す顔料(1秒あたり)
pub const TAKE_RATE: f32 = 1.0;
/// 筆が含める顔料の上限(標準の4回分)
pub const PIGMENT_MAX: f32 = 4.0;
/// 水スウォッチを押した瞬間に残る顔料の割合
pub const DIP_KEEP: f32 = 0.5;
/// 水に浸している間に顔料が抜ける時定数(秒)
pub const DIP_TAU: f32 = 0.25;
/// 顔料の合計がこれを下回ったら 0 に切る(水に浸し続ければ必ず透明になる)
pub const CLEAN_EPS: f32 = 0.02;
/// 水がこれを下回ったら置いたまま流れ出る色水(feed)を止める(brush.wgsl / splat.wgsl がプレリュード経由で使う)
pub const FEED_MIN_WATER: f32 = 0.02;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushLoad {
    /// 含んでいる水(1 = 満タン)
    pub water: f32,
    /// 顔料スロットごとの量(1 = 標準の1回分)
    pub pigment: [f32; 4],
}

impl Default for BrushLoad {
    fn default() -> Self {
        Self::empty()
    }
}

impl BrushLoad {
    /// 空の筆(洗った直後・起動時): 水も顔料もなく、塗っても何も置かない
    pub fn empty() -> Self {
        Self {
            water: 0.0,
            pigment: [0.0; 4],
        }
    }

    /// 水だけの筆: 水満タン・顔料なし
    pub fn water_only() -> Self {
        Self {
            water: 1.0,
            pigment: [0.0; 4],
        }
    }

    /// 空の筆に色を1回分取った状態(スロット ch の標準の濃さ)
    pub fn standard(channel: u32) -> Self {
        let mut load = Self::empty();
        load.take_pigment(channel, TAKE_DOSE);
        load
    }

    pub fn pigment_total(&self) -> f32 {
        self.pigment.iter().sum()
    }

    /// 濃さ = 顔料 ÷ 水。水がほぼ無いときは 0
    pub fn concentration(&self) -> f32 {
        if self.water <= FEED_MIN_WATER {
            0.0
        } else {
            self.pigment_total() / self.water
        }
    }

    /// 色を取る: 顔料を amount 足し、水を満たす(絵具皿の色は水に溶けている)。
    /// 合計は PIGMENT_MAX で頭打ち(比率を保って縮める)
    pub fn take_pigment(&mut self, channel: u32, amount: f32) {
        self.pigment[channel.min(3) as usize] += amount.max(0.0);
        self.water = 1.0;
        let total = self.pigment_total();
        if total > PIGMENT_MAX {
            let k = PIGMENT_MAX / total;
            for p in &mut self.pigment {
                *p *= k;
            }
        }
    }

    /// 水に浸す: 水を満たし、顔料を keep の割合だけ残す。しきい値以下は 0 に切る
    pub fn dip_water(&mut self, keep: f32) {
        self.water = 1.0;
        let keep = keep.clamp(0.0, 1.0);
        for p in &mut self.pigment {
            *p *= keep;
        }
        if self.pigment_total() < CLEAN_EPS {
            self.pigment = [0.0; 4];
        }
    }

    /// 水に dt 秒浸したときに残る顔料の割合
    pub fn dip_keep(dt: f32) -> f32 {
        (-dt.max(0.0) / DIP_TAU).exp()
    }

    pub fn wash(&mut self) {
        *self = Self::empty();
    }
}

/// GPU の筆バッファ(brush.wgsl / splat.wgsl の `Brush`)と同レイアウト(32 バイト)。
/// 描いている間の中身(なぞって減る・置いたまま流れ出る・紙から吸い込む)は GPU 側で進む
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct BrushState {
    pub water: f32,
    pub _pad: [f32; 3],
    pub pigment: [f32; 4],
}

impl From<BrushLoad> for BrushState {
    fn from(load: BrushLoad) -> Self {
        Self {
            water: load.water,
            _pad: [0.0; 3],
            pigment: load.pigment,
        }
    }
}

impl From<BrushState> for BrushLoad {
    fn from(state: BrushState) -> Self {
        Self {
            water: state.water,
            pigment: state.pigment,
        }
    }
}

/// 1フレーム分の、GPU の筆の中身への指示。
/// set はフレーム頭で中身を置き換え、scale と feed_dt は splat を置いた後に brush.wgsl が反映する
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushFrame {
    /// 中身をこれに置き換える(色・水・洗う・自動補充・再生のストローク開始)
    pub set: Option<BrushLoad>,
    /// このフレームのなぞりで中身が残る割合
    pub scale: f32,
    /// 置いたまま流れ出た色水の時間(秒)
    pub feed_dt: f32,
}

impl Default for BrushFrame {
    fn default() -> Self {
        Self {
            set: None,
            scale: 1.0,
            feed_dt: 0.0,
        }
    }
}

impl BrushFrame {
    /// 中身を置き換える。それより前のなぞり・含みは置き換え前の中身の話なので捨てる
    pub fn set_load(&mut self, load: BrushLoad) {
        *self = Self {
            set: Some(load),
            ..Self::default()
        };
    }

    /// 塗る筆の splat 列に「置く時点で残っている割合」を載せ、なぞった長さぶん減らす。
    /// last は直前の splat 位置(ストロークの始めに None へ戻す)。
    /// hold = なぞると中身が半分になる長さ(px)。0 以下なら減らない
    pub fn drag(&mut self, splats: &mut [Splat], last: &mut Option<[f32; 2]>, hold: f32) {
        for s in splats {
            s.scale = self.scale;
            let step = last.map_or(0.0, |p| {
                let (dx, dy) = (s.pos[0] - p[0], s.pos[1] - p[1]);
                (dx * dx + dy * dy).sqrt()
            });
            *last = Some(s.pos);
            if hold > 0.0 && step > 0.0 {
                self.scale *= 0.5f32.powf(step / hold);
            }
        }
    }

    /// 置いたまま動かない間の色水(feed splat)。満タンの筆が charge 秒で空になる速さで、
    /// GPU が筆の水を使う(水が FEED_MIN_WATER 以下なら GPU 側で何もしない)。charge が 0 なら None
    pub fn feed(&mut self, pos: [f32; 2], pressure: f32, dt: f32, charge: f32) -> Option<Splat> {
        if charge <= 0.0 {
            return None;
        }
        self.feed_dt += dt.max(0.0);
        Some(Splat::feed_at(pos, pressure))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_is_one_dose_of_one_color() {
        let load = BrushLoad::standard(2);
        assert_eq!(load.water, 1.0);
        assert_eq!(load.pigment, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(load.concentration(), 1.0);
    }

    /// 色を足すと濃くなり、水に浸すと薄くなる
    #[test]
    fn take_and_dip_change_concentration() {
        let mut load = BrushLoad::standard(1);
        load.take_pigment(1, 1.0);
        assert_eq!(load.concentration(), 2.0);
        load.dip_water(DIP_KEEP);
        assert_eq!(load.concentration(), 1.0);
    }

    /// 2色を取ると筆の中で混ざる(比率は取った量のとおり)
    #[test]
    fn two_colors_mix_in_brush() {
        let mut load = BrushLoad::standard(1);
        load.take_pigment(3, 1.0);
        assert_eq!(load.pigment, [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn pigment_is_capped() {
        let mut load = BrushLoad::standard(0);
        load.take_pigment(1, 10.0);
        assert!((load.pigment_total() - PIGMENT_MAX).abs() < 1e-5);
        assert!(load.pigment[1] > load.pigment[0]);
    }

    /// いちばん濃い筆でも水を 1.2 秒ほど押し続ければ、しきい値で切られて顔料はちょうど 0 になる
    #[test]
    fn long_dip_cleans_completely() {
        let mut load = BrushLoad::standard(0);
        load.take_pigment(0, 3.0);
        load.dip_water(DIP_KEEP);
        for _ in 0..72 {
            load.dip_water(BrushLoad::dip_keep(1.0 / 60.0));
        }
        assert_eq!(load.pigment, [0.0; 4]);
        assert_eq!(load.water, 1.0);
    }

    /// 洗うと空になり、水に浸すと水だけの筆になる
    #[test]
    fn wash_empties_then_dip_gives_water_only() {
        let mut load = BrushLoad::standard(2);
        load.wash();
        assert_eq!(load, BrushLoad::empty());
        assert_eq!(load.concentration(), 0.0);
        load.dip_water(DIP_KEEP);
        assert_eq!(load, BrushLoad::water_only());
    }

    /// なぞると hold px で半分になる。各 splat には置く時点で残っている割合が載る
    #[test]
    fn drag_scales_splats_and_frame() {
        let mut splats = vec![
            Splat::new([0.0, 0.0], [0.0, 0.0], 1.0),
            Splat::new([100.0, 0.0], [1.0, 0.0], 1.0),
            Splat::new([200.0, 0.0], [1.0, 0.0], 1.0),
        ];
        let mut frame = BrushFrame::default();
        let mut last = None;
        frame.drag(&mut splats, &mut last, 100.0);
        assert_eq!(splats[0].scale, 1.0);
        assert_eq!(splats[1].scale, 1.0);
        assert!((splats[2].scale - 0.5).abs() < 1e-5);
        assert!((frame.scale - 0.25).abs() < 1e-5);
        assert_eq!(last, Some([200.0, 0.0]));
    }

    #[test]
    fn hold_zero_never_depletes() {
        let mut splats = vec![
            Splat::new([0.0, 0.0], [0.0, 0.0], 1.0),
            Splat::new([500.0, 0.0], [1.0, 0.0], 1.0),
        ];
        let mut frame = BrushFrame::default();
        frame.drag(&mut splats, &mut None, 0.0);
        assert_eq!(frame.scale, 1.0);
    }

    /// 中身を置き換えると、それより前のなぞり・含みは捨てる
    #[test]
    fn set_load_resets_frame() {
        let mut frame = BrushFrame::default();
        frame.drag(
            &mut [
                Splat::new([0.0, 0.0], [0.0, 0.0], 1.0),
                Splat::new([50.0, 0.0], [1.0, 0.0], 1.0),
            ],
            &mut None,
            10.0,
        );
        assert!(frame.feed([0.0, 0.0], 1.0, 0.1, 2.0).is_some());
        frame.set_load(BrushLoad::standard(1));
        assert_eq!(frame.scale, 1.0);
        assert_eq!(frame.feed_dt, 0.0);
        assert_eq!(frame.set, Some(BrushLoad::standard(1)));
    }

    /// 置いたままの色水は経過時間を積み、含み 0 なら出さない
    #[test]
    fn feed_accumulates_time() {
        let mut frame = BrushFrame::default();
        assert!(frame.feed([0.0, 0.0], 1.0, 0.1, 2.0).is_some());
        assert!(frame.feed([0.0, 0.0], 1.0, 0.1, 2.0).unwrap().feed > 0.5);
        assert!((frame.feed_dt - 0.2).abs() < 1e-6);
        assert!(frame.feed([0.0, 0.0], 1.0, 0.1, 0.0).is_none());
    }

    /// GPU レイアウトとの往復で中身が変わらない
    #[test]
    fn gpu_state_roundtrip() {
        let mut load = BrushLoad::standard(2);
        load.take_pigment(0, 0.5);
        assert_eq!(std::mem::size_of::<BrushState>(), 32);
        assert_eq!(BrushLoad::from(BrushState::from(load)), load);
    }
}
