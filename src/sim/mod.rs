//! シミュレーション制御の型: SimParams(H2)と splat 入力。
//!
//! テクスチャ構成(いずれも rgba32float、ping-pong 2枚組):
//! - 水: r=水量 / g=速度x / b=速度y / a=濡れマスク(wet-area mask)
//! - 浮遊顔料(M1b): rgba の各チャンネル = 顔料1種(M1c から4顔料。pigment.rs の PIGMENTS と対応)
//! - 沈着顔料(M1b): 同上。紙に定着した分で、移流しない
//!
//! 濡れマスクは筆が通ったセルで 1。水と顔料が動くのはマスク内だけで、
//! 乾いた紙との境界は壁として扱う(にじみがストローク領域の外へ広がらない)。
//! これに加えて紙ハイトテクスチャ(M1d、r32float 1枚。ping-pong しない静的な紙の凹凸。
//! 生成は paper.rs)があり、速度勾配・にじみ拡張・吸着の3箇所を変調する。
//! 1 シミュレーションステップのパス順序は gpu/mod.rs の prepare() 参照:
//!   splat(水+初速+顔料の注入)→ 速度更新 → 発散緩和 × relax_iters
//!   → FlowOutward(エッジダークニング、edge_eta > 0 のとき)→ 移流(水+浮遊顔料)
//!   → 顔料拡散 × diffuse_iters → 吸着/脱着+蒸発(transfer)

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// キャンバス(=シミュレーション)解像度。plan.md の決定に従い 512²。
pub const CANVAS_SIZE: u32 = 512;

/// 1フレームで GPU に送る splat の上限(storage buffer の固定長)
pub const MAX_SPLATS: usize = 1024;

/// 全シミュレーションパラメータの唯一の置き場(H2)。
/// フィールドを足したら: ①ここに1行 ②app.rs のスライダー1行 ③common.wgsl の struct に1行。
/// メモリレイアウトは WGSL の uniform 規則(16 バイト整列)に合わせること。
/// 全体サイズは 16 の倍数を保つ(末尾の _pad を置き換えてからフィールドを増やす)。
/// serde(default): プリセット(H3)にないフィールドは Default 値で埋める
/// (パラメータを増やしても古い JSON が読めるように)。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable, Serialize, Deserialize)]
#[serde(default)]
pub struct SimParams {
    /// ブラシ半径(テクセル単位)
    pub brush_radius: f32,
    /// 1 splat あたりに置く水量
    pub brush_water: f32,
    /// splat 時にストローク方向へ与える初速
    pub brush_velocity: f32,
    /// シミュレーションの時間刻み
    pub dt: f32,
    /// 移流強度: 水面勾配 → 加速度の係数
    pub accel: f32,
    /// 速度減衰(粘性の代用。1ステップあたりの減衰率)
    pub damping: f32,
    /// 発散緩和の係数 ξ(Curtis 1997 の既定は 0.1)
    pub xi: f32,
    /// 発散緩和の反復回数(1ステップあたり)
    pub relax_iters: u32,
    /// 速度上限(セル/ステップ)。CFL 的制約として 1.0 以下を推奨
    pub vel_max: f32,
    /// デバッグ表示モード(H4): 0=通常 / 1=水量ヒートマップ / 2=速度場 / 3=湿りオーバーレイ
    pub display_mode: u32,
    /// デバッグ表示の輝度スケール
    pub display_gain: f32,
    /// にじみ拡張率: 濡れた隣の水量に比例して乾いたセルのマスクが育つ速さ(0=固定マスク)
    pub wet_expand: f32,
    /// 1 splat あたりに置く顔料量(0 = 水だけのブラシ)
    pub brush_pigment: f32,
    /// 吸着率: 浮遊顔料が紙へ沈着する速さ(水が少ないほど強く効く)
    pub deposit_rate: f32,
    /// 脱着率: 沈着顔料が水に浮き上がる速さ(水が多いほど強く効く)
    pub lift_rate: f32,
    /// 蒸発率: 濡れ領域の水が 1 ステップに減る量
    pub evap_rate: f32,
    /// 顔料拡散率: 浮遊顔料が水の中をにじんで広がる速さ(1反復あたり。安定のため WGSL 側で 0.2 に制限)
    pub pigment_diffuse: f32,
    /// 顔料拡散の反復回数(1ステップあたり)。実効的なにじみ速度 = 拡散率 × 反復回数。
    /// 水筆で描いた水路に顔料溜まりを接続したとき、色が水路へ広がる速さはここで稼ぐ
    pub diffuse_iters: u32,
    /// ブラシで注入する顔料スロット(0..3 = pigment.rs の PIGMENTS と対応)
    pub brush_channel: u32,
    /// 発色の濃さ: 顔料濃度 → 被覆率(1-exp(-density·濃度))の係数(display.wgsl)
    pub pigment_density: f32,
    /// 紙ハイトの振幅: 水面勾配に足す紙の凹凸の強さ(水が紙の谷へ流れる → ストリーク)
    pub paper_amp: f32,
    /// 粒状化: 紙の凹部で吸着が強まる度合い(0=紙目の影響なし)
    pub paper_gran: f32,
    /// にじみ拡張の紙目変調: 濡れ前線が紙の谷を選んで進む度合い(0=一様に広がる)
    pub paper_wet: f32,
    /// FlowOutward の η: 濡れ領域の縁ほど水を除去する強さ(0=エッジダークニングなし)。
    /// 既定 0 で先送り(status.md 参照): 今の弱い定式化では顔料が縁でなく中心へ寄るため、
    /// ちゃんとした乾燥が入る M2 で再検討する。パス・スライダーは残してある(オフならゼロコスト)
    pub edge_eta: f32,
    /// FlowOutward の濡れマスクぼかし半径(テクセル。縁と判定する帯の幅。WGSL 側で 1..8 に制限)
    pub edge_radius: u32,
    /// 筆圧→半径の効き(M1.5)。0=筆圧無効 / 1=筆圧に完全比例。
    /// 実効値 = 基準値 × mix(1, 筆圧^γ, 効き)。マウス(筆圧 1.0)では常に基準値のまま
    pub pressure_radius: f32,
    /// 筆圧→水量の効き(同上)
    pub pressure_water: f32,
    /// 筆圧→顔料量の効き(同上)
    pub pressure_pigment: f32,
    /// 筆圧の応答カーブ γ(筆圧^γ)。1=線形 / >1 で軽いタッチがより細く弱くなる
    pub pressure_gamma: f32,
    /// 乾燥シフト(M2): 焼き込み時に顔料濃度へ掛ける係数。水彩は乾くと薄くなる(<1)
    pub dry_shift: f32,
    /// 焼き込み時の粒状感ゲート(M2): 紙の凹部で濃く/凸部で薄く定着する度合い(0=無効)。
    /// transfer.wgsl の paper_gran(描画中の吸着変調)とは独立に、乾く瞬間の紙目を強調する
    pub dry_gran: f32,
    /// 焼き込み時のエッジダークニング(M2): 顔料の縁バンドで濃度を増す強さ(0=無効)。
    /// Curtis のエッジダークニングは乾燥時に起こる現象なので定着パスで掛ける。
    /// 縁バンドは顔料被覆率のぼかし残差(濡れマスク基準だと縁の濃度がゼロで効かない)
    /// (M1d の FlowOutward = シミュレーション中の縁への移流とは別方式。縁バンド幅は edge_radius を共用)
    pub dry_edge: f32,
    /// Wet the Layer(M2): 再湿潤で全面に足す水量
    pub rewet_water: f32,
    /// uniform の 16 バイト境界合わせ。パラメータを増やすときはまずここを置き換える
    #[serde(skip)]
    pub _pad0: u32,
    #[serde(skip)]
    pub _pad1: u32,
    #[serde(skip)]
    pub _pad2: u32,
}

impl SimParams {
    /// 筆圧を反映した実効ブラシ半径(splat.wgsl と同じ式)。
    /// CPU 側ではストローク補間のサンプル間隔の算出に使う(brush.rs / replay.rs)
    pub fn radius_at(&self, pressure: f32) -> f32 {
        let p = pressure
            .clamp(0.0, 1.0)
            .powf(self.pressure_gamma.max(0.01));
        let factor = 1.0 + (p - 1.0) * self.pressure_radius.clamp(0.0, 1.0);
        (self.brush_radius * factor).max(0.5)
    }
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            brush_radius: 16.0,
            brush_water: 0.5,
            brush_velocity: 0.5,
            dt: 0.5,
            accel: 1.0,
            damping: 0.05,
            xi: 0.1,
            relax_iters: 16,
            vel_max: 1.0,
            display_mode: 0,
            display_gain: 1.0,
            wet_expand: 0.0,
            brush_pigment: 0.6,
            deposit_rate: 0.05,
            lift_rate: 0.02,
            evap_rate: 0.005,
            pigment_diffuse: 0.15,
            diffuse_iters: 4,
            brush_channel: 0,
            pigment_density: 3.0,
            paper_amp: 0.3,
            paper_gran: 0.4,
            paper_wet: 0.5,
            edge_eta: 0.0,
            edge_radius: 4,
            pressure_radius: 0.6,
            pressure_water: 0.3,
            pressure_pigment: 0.7,
            pressure_gamma: 1.0,
            dry_shift: 0.85,
            dry_gran: 0.0,
            dry_edge: 0.4,
            rewet_water: 0.5,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        }
    }
}

/// ストローク上の1点。WGSL 側の Splat と同レイアウト(32 バイト)。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Splat {
    /// テクセル座標
    pub pos: [f32; 2],
    /// ストローク方向の単位ベクトル(初速の向き。ストローク始点は 0)
    pub vel: [f32; 2],
    /// 筆圧(マウスは 1.0。M1.5 で octotablet の値が入る)
    pub pressure: f32,
    pub _pad: [f32; 3],
}

impl Splat {
    pub fn new(pos: [f32; 2], vel: [f32; 2], pressure: f32) -> Self {
        Self {
            pos,
            vel,
            pressure,
            _pad: [0.0; 3],
        }
    }
}

/// splat storage buffer の先頭 16 バイト
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SplatHeader {
    pub count: u32,
    pub _pad: [u32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 筆圧→半径マッピング(M1.5): 筆圧 1.0(マウス)は常に基準半径、
    /// 効き 1.0 + γ=1 で完全比例、効き 0 で筆圧を無視
    #[test]
    fn radius_at_pressure() {
        let mut params = SimParams {
            brush_radius: 20.0,
            pressure_radius: 1.0,
            pressure_gamma: 1.0,
            ..SimParams::default()
        };
        assert_eq!(params.radius_at(1.0), 20.0);
        assert_eq!(params.radius_at(0.5), 10.0);
        params.pressure_radius = 0.0;
        assert_eq!(params.radius_at(0.0), 20.0);
    }

    /// SimParams は WGSL uniform 規則(16 バイト整列)に合わせてサイズが 16 の倍数
    #[test]
    fn params_size_aligned() {
        assert_eq!(std::mem::size_of::<SimParams>() % 16, 0);
    }
}
