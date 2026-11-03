//! 筆の中身(含んでいる水と顔料)。
//!
//! 単位は「標準の1回分」: 水 1 = 満タン、顔料 1 = 満タンの水に色1回分を溶かした量。
//! 濃さ = 顔料の合計 ÷ 水。
//!
//! 中身の正典は GPU の筆バッファ([`BrushState`])。筆と紙は濡れ具合の差で水を受け渡し
//! (筆が濡れていれば紙へ出て、紙が濡れていれば筆へ入る。顔料は水と同じ割合で一緒に動く)、
//! splat.wgsl が受け渡した量を集計して、brush.wgsl がフレームの最後に中身へ反映する。
//! CPU は [`BrushFrame`] で「中身を置き換える」と「splat ごとの残りの予測」を毎フレーム渡し、
//! 読み戻した中身を表示と次の操作の起点に使う。
//!
//! 中身を変えるのはツールバーのスウォッチとスポイト:
//! - 色: 押した瞬間にその色を1回分足す(筆の中で混ざる。Shift なら顔料を捨ててから取る持ち替え)、押している間も少しずつ足す(濃くなる)。
//!   水は変えない(水の量は水と拭うで決める)。水が TAKE_MIN_WATER より少なければそこまでは入れる
//! - 水: 押した瞬間に顔料を半分、押している間も水と入れ替わって抜けていく(薄くなる・洗える)
//! - 拭う: 押した瞬間に水を半分、押している間も水だけ抜けていく(顔料は残るので濃く、紙を濡らす量が減る)
//! - 洗う: 水も顔料も 0(空の筆。紙に何も置かず、濡れた所では色水を吸う)。続けて水に浸せば水だけの筆になる
//! - スポイト: 顔料を捨ててから、拾った点の湿レイヤーの顔料構成の比で合計1回分を取る([`BrushLoad::take_mix`])。
//!   水は拾った所の紙の濡れ具合まで増やす([`BrushLoad::fill_water_to`])
//!
//! 筆は穂先(紙と受け渡す。小さい)と腹(穂先へ水を送る。大きい)の2室。乾いた紙に描くと穂先から出て、
//! 腹が補充するので全体が減っていく(腹 `brush_hold` px で半分)。出るときは穂先の濃さのまま出るので濃さは
//! 変わらない。紙のほうが濡れている所では穂先が水と浮いている色を吸うので、乾きかけの筆で濡れた色を
//! なでると穂先がその色で満ち、その色を運んで置ける。

use crate::sim::{SimParams, Splat};
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// 色スウォッチを押した瞬間に足す顔料(標準の1回分)
pub const TAKE_DOSE: f32 = 1.0;
/// 色スウォッチを押し続けている間に足す顔料(1秒あたり)
pub const TAKE_RATE: f32 = 1.0;
/// 色を取ったときに入れる水の下限(拭い切った筆と同じ量)。水 0 の筆は乾いた紙に色を出せないので、空の筆に色を取ってもここまでは入れる
pub const TAKE_MIN_WATER: f32 = WIPE_MIN_WATER;
/// 筆が含める顔料の上限(標準の4回分)
pub const PIGMENT_MAX: f32 = 4.0;
/// 水スウォッチを押した瞬間に残る顔料の割合
pub const DIP_KEEP: f32 = 0.5;
/// 水に浸している間に顔料が抜ける時定数(秒)
pub const DIP_TAU: f32 = 0.25;
/// 拭うボタンを押した瞬間に残る水の割合
pub const WIPE_KEEP: f32 = 0.5;
/// 拭っている間に水が抜ける時定数(秒)
pub const WIPE_TAU: f32 = 0.5;
/// 拭って残る水の下限。これより乾かすと紙へほとんど出なくなり、濃さだけが際限なく上がるので止める
pub const WIPE_MIN_WATER: f32 = 0.1;
/// 顔料の合計がこれを下回ったら 0 に切る(水に浸し続ければ必ず透明になる)
pub const CLEAN_EPS: f32 = 0.02;
/// 水がこれ以下の室は、濃さ(顔料 ÷ 水)を 0 として表示する(ほぼ空の室で濃さが跳ねないように)
pub const FEED_MIN_WATER: f32 = 0.02;

/// 筆の1室(穂先または腹)の中身。水 1 = その室が満タン、顔料 1 = 満タンの水に標準の1回分
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chamber {
    pub water: f32,
    pub pigment: [f32; 4],
}

impl Chamber {
    const EMPTY: Self = Self {
        water: 0.0,
        pigment: [0.0; 4],
    };

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

    fn take_pigment(&mut self, channel: u32, amount: f32) {
        let mut add = [0.0; 4];
        add[channel.min(3) as usize] = amount.max(0.0);
        self.add_pigment(add);
    }

    /// 顔料を足す。水は変えず、TAKE_MIN_WATER より少なければそこまで入れる。合計は PIGMENT_MAX で頭打ち(比率を保って縮める)
    fn add_pigment(&mut self, add: [f32; 4]) {
        for (p, a) in self.pigment.iter_mut().zip(add) {
            *p += a;
        }
        self.water = self.water.max(TAKE_MIN_WATER);
        let total = self.pigment_total();
        if total > PIGMENT_MAX {
            let k = PIGMENT_MAX / total;
            for p in &mut self.pigment {
                *p *= k;
            }
        }
    }

    fn dip_water(&mut self, keep: f32) {
        self.water = 1.0;
        let keep = keep.clamp(0.0, 1.0);
        for p in &mut self.pigment {
            *p *= keep;
        }
        if self.pigment_total() < CLEAN_EPS {
            self.pigment = [0.0; 4];
        }
    }

    fn wipe(&mut self, keep: f32) {
        if self.water > WIPE_MIN_WATER {
            self.water = (self.water * keep.clamp(0.0, 1.0)).max(WIPE_MIN_WATER);
        }
    }
}

/// 塗る筆の中身。腹(`water` / `pigment`。大きく、穂先へ水を送る)と穂先(`tip`。小さく、紙と受け渡す)の2室。
/// 色を取る・水に浸す・洗うは両方の室に同じ操作をするので、作った直後の2室は同じ濡れ具合・同じ濃さになる。
/// 描いている間に穂先だけが紙と受け渡して腹と違ってくる
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushLoad {
    /// 腹の水(1 = 満タン)
    pub water: f32,
    /// 腹の顔料スロットごとの量(1 = 標準の1回分)
    pub pigment: [f32; 4],
    /// 穂先。None = 腹と同じ(色を作った直後。穂先の無い古い記録・保存もこれとして読む)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip: Option<Chamber>,
}

impl Default for BrushLoad {
    fn default() -> Self {
        Self::empty()
    }
}

impl BrushLoad {
    /// 空の筆(洗った直後・起動時): 水も顔料もない。紙に何も置かず、濡れた所の色水を吸うだけ
    pub fn empty() -> Self {
        Self::uniform(Chamber::EMPTY)
    }

    /// 水だけの筆: 水満タン・顔料なし
    pub fn water_only() -> Self {
        Self::uniform(Chamber {
            water: 1.0,
            pigment: [0.0; 4],
        })
    }

    /// 穂先と腹が同じ中身の筆
    pub fn uniform(chamber: Chamber) -> Self {
        Self::from_chambers(chamber, chamber)
    }

    /// 穂先と腹から組み立てる(同じなら穂先は None にまとめる)
    pub fn from_chambers(tip: Chamber, belly: Chamber) -> Self {
        Self {
            water: belly.water,
            pigment: belly.pigment,
            tip: (tip != belly).then_some(tip),
        }
    }

    /// 水満タンの筆に色を1回分取った状態(スロット ch の標準の濃さ)
    pub fn standard(channel: u32) -> Self {
        let mut load = Self::water_only();
        load.take_pigment(channel, TAKE_DOSE);
        load
    }

    pub fn belly(&self) -> Chamber {
        Chamber {
            water: self.water,
            pigment: self.pigment,
        }
    }

    pub fn tip(&self) -> Chamber {
        self.tip.unwrap_or(self.belly())
    }

    /// 腹の顔料の合計
    pub fn pigment_total(&self) -> f32 {
        self.belly().pigment_total()
    }

    /// 次に置く色の濃さ(穂先の顔料 ÷ 穂先の水)
    pub fn concentration(&self) -> f32 {
        self.tip().concentration()
    }

    /// 穂先と腹に同じ操作をする
    fn each(&mut self, op: impl Fn(&mut Chamber)) {
        let (mut tip, mut belly) = (self.tip(), self.belly());
        op(&mut tip);
        op(&mut belly);
        *self = Self::from_chambers(tip, belly);
    }

    /// 色を取る: 顔料を amount 足す。水は変えない(TAKE_MIN_WATER より少なければそこまで入れる)。
    /// 合計は PIGMENT_MAX で頭打ち(比率を保って縮める)
    pub fn take_pigment(&mut self, channel: u32, amount: f32) {
        self.each(|c| c.take_pigment(channel, amount));
    }

    /// 配合比 `ratio`(4スロット。合計は問わない、負は 0 扱い)で顔料を合計 `amount` 取る。
    /// 比がすべて 0 なら何もしない
    pub fn take_mix(&mut self, ratio: [f32; 4], amount: f32) {
        let sum: f32 = ratio.iter().map(|r| r.max(0.0)).sum();
        if sum <= 0.0 {
            return;
        }
        let total = amount.max(0.0);
        let add = ratio.map(|r| total * r.max(0.0) / sum);
        self.each(|c| c.add_pigment(add));
    }

    /// 水を level(0〜1)まで増やす。すでに多ければそのまま
    pub fn fill_water_to(&mut self, level: f32) {
        let level = level.clamp(0.0, 1.0);
        self.each(|c| c.water = c.water.max(level));
    }

    /// 顔料だけを捨てる(水は残す)。色を持ち替えるときに使う
    pub fn clear_pigment(&mut self) {
        self.each(|c| c.pigment = [0.0; 4]);
    }

    /// 水に浸す: 水を満たし、顔料を keep の割合だけ残す。しきい値以下は 0 に切る
    pub fn dip_water(&mut self, keep: f32) {
        self.each(|c| c.dip_water(keep));
    }

    /// 水に dt 秒浸したときに残る顔料の割合
    pub fn dip_keep(dt: f32) -> f32 {
        (-dt.max(0.0) / DIP_TAU).exp()
    }

    /// 拭う: 水を keep の割合だけ残す(顔料は残すので濃くなる)。水は WIPE_MIN_WATER より下げない
    pub fn wipe(&mut self, keep: f32) {
        self.each(|c| c.wipe(keep));
    }

    /// dt 秒拭ったときに残る水の割合
    pub fn wipe_keep(dt: f32) -> f32 {
        (-dt.max(0.0) / WIPE_TAU).exp()
    }

    pub fn wash(&mut self) {
        *self = Self::empty();
    }
}

/// GPU の筆バッファ(brush.wgsl / splat.wgsl の `Brush`)と同レイアウト(64 バイト)。
/// 描いている間の中身(紙との受け渡し・腹から穂先への補充)は GPU 側で進む
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct BrushState {
    pub tip_water: f32,
    pub _pad0: [f32; 3],
    pub tip_pigment: [f32; 4],
    pub belly_water: f32,
    pub _pad1: [f32; 3],
    pub belly_pigment: [f32; 4],
}

impl From<BrushLoad> for BrushState {
    fn from(load: BrushLoad) -> Self {
        let tip = load.tip();
        Self {
            tip_water: tip.water,
            _pad0: [0.0; 3],
            tip_pigment: tip.pigment,
            belly_water: load.water,
            _pad1: [0.0; 3],
            belly_pigment: load.pigment,
        }
    }
}

impl From<BrushState> for BrushLoad {
    fn from(state: BrushState) -> Self {
        Self::from_chambers(
            Chamber {
                water: state.tip_water,
                pigment: state.tip_pigment,
            },
            Chamber {
                water: state.belly_water,
                pigment: state.belly_pigment,
            },
        )
    }
}

/// 1フレーム分の、GPU の筆の中身への指示。
/// set はフレーム頭で中身を置き換える。scale は splat ごとの穂先の残りの予測を前から掛けていく途中の値。
/// dt は brush.wgsl が腹から穂先へ補充する時間(前回の brush パスからの経過。[`BrushClock`])
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushFrame {
    /// 中身をこれに置き換える(再生のストローク開始)
    pub set: Option<BrushLoad>,
    /// このフレームでここまでに置いた splat で、穂先が最も減った場合に残る割合
    pub scale: f32,
    /// 腹から穂先へ補充する時間(秒)
    pub dt: f32,
}

impl Default for BrushFrame {
    fn default() -> Self {
        Self {
            set: None,
            scale: 1.0,
            dt: 0.0,
        }
    }
}

impl BrushFrame {
    /// 中身を置き換える。それより前の予測は置き換え前の中身の話なので捨てる(補充の時間は残す)
    pub fn set_load(&mut self, load: BrushLoad) {
        *self = Self {
            set: Some(load),
            dt: self.dt,
            ..Self::default()
        };
    }

    /// 塗る筆の splat 列に「置く時点で穂先に残っている割合」の予測を載せる。予測は乾いた紙に置き、
    /// 腹からの補充も無い場合(= 穂先が最も減る場合)なので、フレームの中で穂先に残っている以上を出すことはない。
    /// 実際の中身はフレームの最後に GPU が受け渡しの集計と補充から確定する
    pub fn drag(&mut self, splats: &mut [Splat], params: &SimParams) {
        for s in splats {
            s.scale = self.scale;
            self.scale *= 1.0 - params.brush_release_max(s.pressure);
        }
    }
}

/// 腹から穂先へ補充する時間を数える。brush パスは塗るツールで splat があるフレームにしか走らないので、
/// それ以外のフレーム(筆を上げている間など)の刻みも溜めておき、次に塗るフレームでまとめて渡す
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BrushClock {
    ticks: u32,
}

impl BrushClock {
    /// まとめて補充する時間の上限(刻み)。それ以上待っても穂先はほぼ揃っている
    pub const MAX_TICKS: u32 = 120;

    /// シミュの刻みを進める(1刻み = 1/60 秒)
    pub fn tick(&mut self, ticks: u32) {
        self.ticks = self.ticks.saturating_add(ticks);
    }

    /// brush パスが走るフレームで呼ぶ: 溜めた時間(秒)を返して 0 に戻す
    pub fn take(&mut self) -> f32 {
        let dt = self.ticks.min(Self::MAX_TICKS) as f32 / 60.0;
        self.ticks = 0;
        dt
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

    /// 配合比で合計1回分を取る(比の合計は問わない)
    #[test]
    fn take_mix_splits_one_dose_by_ratio() {
        let mut load = BrushLoad::water_only();
        load.take_mix([0.0, 0.3, 0.0, 0.1], TAKE_DOSE);
        assert_eq!(load.water, 1.0);
        assert!((load.pigment[1] - 0.75).abs() < 1e-6);
        assert!((load.pigment[3] - 0.25).abs() < 1e-6);
        assert!((load.concentration() - 1.0).abs() < 1e-6);
        assert_eq!(load.tip, None);

        let before = load;
        load.take_mix([0.0; 4], TAKE_DOSE);
        assert_eq!(load, before);
    }

    /// 色を取っても水は変わらない。拭ってから持ち替えれば濃い色になり、空の筆でも下限までは水が入る
    #[test]
    fn take_keeps_water() {
        let mut load = BrushLoad::standard(1);
        load.wipe(WIPE_KEEP);
        load.clear_pigment();
        load.take_pigment(2, TAKE_DOSE);
        assert_eq!(load.water, 0.5);
        assert_eq!(load.pigment, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(load.concentration(), 2.0);

        let mut empty = BrushLoad::empty();
        empty.take_pigment(0, TAKE_DOSE);
        assert_eq!(empty.water, TAKE_MIN_WATER);

        let mut picked = BrushLoad::empty();
        picked.take_mix([1.0, 1.0, 0.0, 0.0], TAKE_DOSE);
        assert_eq!(picked.water, TAKE_MIN_WATER);
        assert_eq!(picked.pigment, [0.5, 0.5, 0.0, 0.0]);
    }

    /// 水を増やすだけで減らさない(スポイトが拾った所の濡れ具合に使う)
    #[test]
    fn fill_water_only_raises() {
        let mut load = BrushLoad::standard(0);
        load.wipe(WIPE_KEEP);
        load.fill_water_to(0.8);
        assert_eq!(load.water, 0.8);
        assert_eq!(load.tip().water, 0.8);
        load.fill_water_to(0.3);
        assert_eq!(load.water, 0.8);
        load.fill_water_to(2.0);
        assert_eq!(load.water, 1.0);
        assert_eq!(load.pigment, [1.0, 0.0, 0.0, 0.0]);
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

    /// 拭うと水だけが減って濃くなり、押し続けても下限で止まる。空の筆には水を足さない
    #[test]
    fn wipe_removes_water_and_keeps_pigment() {
        let mut load = BrushLoad::standard(1);
        load.wipe(WIPE_KEEP);
        assert_eq!(load.water, 0.5);
        assert_eq!(load.pigment, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(load.concentration(), 2.0);
        for _ in 0..600 {
            load.wipe(BrushLoad::wipe_keep(1.0 / 60.0));
        }
        assert_eq!(load.water, WIPE_MIN_WATER);
        assert_eq!(load.pigment, [0.0, 1.0, 0.0, 0.0]);

        let mut empty = BrushLoad::empty();
        empty.wipe(WIPE_KEEP);
        assert_eq!(empty, BrushLoad::empty());
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

    /// 各 splat には置く時点で残っている割合の予測が載り、1 splat ごとに乾いた紙での減りの上限だけ減る
    #[test]
    fn drag_predicts_dry_paper_release() {
        let params = SimParams::default();
        let f = params.brush_release_max(1.0);
        assert!(f > 0.0 && f < 0.1, "1 splat の減り {f}");
        let mut splats = vec![Splat::new([0.0, 0.0], [0.0, 0.0], 1.0); 3];
        let mut frame = BrushFrame::default();
        frame.drag(&mut splats, &params);
        assert_eq!(splats[0].scale, 1.0);
        assert!((splats[1].scale - (1.0 - f)).abs() < 1e-6);
        assert!((frame.scale - (1.0 - f).powi(3)).abs() < 1e-6);
    }

    /// 予測は減りの上限なので、乾いた紙を穂先の大きさ(brush_tip)の長さ塗ったときに半分以下になる
    #[test]
    fn prediction_is_upper_bound_of_tip() {
        let params = SimParams::default();
        let spacing = params.radius_at(1.0) * crate::sim::SPLAT_SPACING;
        let n = (params.brush_tip / spacing) as usize;
        let mut splats = vec![Splat::new([0.0, 0.0], [0.0, 0.0], 1.0); n];
        let mut frame = BrushFrame::default();
        frame.drag(&mut splats, &params);
        assert!(frame.scale < 0.5, "予測 {}", frame.scale);
    }

    #[test]
    fn hold_zero_never_depletes() {
        let params = SimParams {
            brush_hold: 0.0,
            ..SimParams::default()
        };
        let mut splats = vec![Splat::new([0.0, 0.0], [0.0, 0.0], 1.0); 2];
        let mut frame = BrushFrame::default();
        frame.drag(&mut splats, &params);
        assert_eq!(frame.scale, 1.0);
    }

    /// 中身を置き換えると、それより前の予測は捨てる
    #[test]
    fn set_load_resets_frame() {
        let mut frame = BrushFrame::default();
        frame.drag(
            &mut [Splat::new([0.0, 0.0], [0.0, 0.0], 1.0)],
            &SimParams::default(),
        );
        assert!(frame.scale < 1.0);
        frame.set_load(BrushLoad::standard(1));
        assert_eq!(frame.scale, 1.0);
        assert_eq!(frame.set, Some(BrushLoad::standard(1)));
    }

    /// GPU レイアウトとの往復で中身が変わらない
    #[test]
    fn gpu_state_roundtrip() {
        let mut load = BrushLoad::standard(2);
        load.take_pigment(0, 0.5);
        assert_eq!(std::mem::size_of::<BrushState>(), 64);
        assert_eq!(BrushLoad::from(BrushState::from(load)), load);
    }
    /// 色を取る・水に浸す・洗うは穂先と腹の両方に効き、穂先が汚れていても同じ操作がかかる
    #[test]
    fn operations_apply_to_both_chambers() {
        let dirty_tip = Chamber {
            water: 0.4,
            pigment: [0.3, 0.0, 0.0, 0.0],
        };
        let mut load = BrushLoad::from_chambers(dirty_tip, BrushLoad::standard(1).belly());
        load.take_pigment(1, 1.0);
        assert_eq!(load.tip().water, 0.4);
        assert_eq!(load.tip().pigment, [0.3, 1.0, 0.0, 0.0]);
        assert_eq!(load.pigment, [0.0, 2.0, 0.0, 0.0]);
        load.wash();
        assert_eq!(load, BrushLoad::empty());
        assert_eq!(load.tip, None);
    }

    /// 穂先の無い古い記録・保存は「穂先 = 腹」として読み、同じなら書くときも省く
    #[test]
    fn json_without_tip_reads_as_uniform() {
        let load: BrushLoad =
            serde_json::from_str(r#"{"water":1.0,"pigment":[0.0,1.0,0.0,0.0]}"#).unwrap();
        assert_eq!(load, BrushLoad::standard(1));
        assert_eq!(load.tip(), load.belly());
        assert!(!serde_json::to_string(&load).unwrap().contains("tip"));
    }

    /// 補充の時間は塗らない間も溜まり、上限で頭打ちして、取り出すと 0 に戻る
    #[test]
    fn brush_clock_accumulates_and_caps() {
        let mut clock = BrushClock::default();
        clock.tick(30);
        assert!((clock.take() - 0.5).abs() < 1e-6);
        assert_eq!(clock.take(), 0.0);
        clock.tick(1000);
        assert!((clock.take() - BrushClock::MAX_TICKS as f32 / 60.0).abs() < 1e-6);
    }
}
