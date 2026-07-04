//! シミュレーション制御の型: SimParams(H2)と splat 入力。
//!
//! テクスチャ構成(いずれも rgba32float、ping-pong 2枚組):
//! - 水: r=水量 / g=速度x / b=速度y / a=濡れマスク(wet-area mask)
//! - 浮遊顔料(M1b): rgba の各チャンネル = 顔料1種(M1b は r のみ、M1c で4顔料化)
//! - 沈着顔料(M1b): 同上。紙に定着した分で、移流しない
//!
//! 濡れマスクは筆が通ったセルで 1。水と顔料が動くのはマスク内だけで、
//! 乾いた紙との境界は壁として扱う(にじみがストローク領域の外へ広がらない)。
//! 1 シミュレーションステップのパス順序は gpu/mod.rs の prepare() 参照:
//!   splat(水+初速+顔料の注入)→ 速度更新 → 発散緩和 × relax_iters → 移流(水+浮遊顔料)
//!   → 顔料拡散 × diffuse_iters → 吸着/脱着+蒸発(transfer)
//! M1d で紙ハイトテクスチャをここに足す。

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// キャンバス(=シミュレーション)解像度。plan.md の決定に従い 512²。
pub const CANVAS_SIZE: u32 = 512;

/// 1フレームで GPU に送る splat の上限(storage buffer の固定長)
pub const MAX_SPLATS: usize = 1024;

/// 全シミュレーションパラメータの唯一の置き場(H2)。
/// フィールドを足したら: ①ここに1行 ②app.rs のスライダー1行 ③common.wgsl の struct に1行。
/// メモリレイアウトは WGSL の uniform 規則(16 バイト整列)に合わせること。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable, Serialize, Deserialize)]
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
