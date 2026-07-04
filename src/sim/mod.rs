//! シミュレーション制御。M0 では SimParams(H2)と splat 入力の型のみ。
//! M1 でテクスチャ群(水量+速度 / 浮遊顔料 / 沈着顔料 / 紙ハイト)とパス順序をここに足す。

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// キャンバス(=シミュレーション)解像度。plan.md の決定に従い 512²。
pub const CANVAS_SIZE: u32 = 512;

/// 1フレームで GPU に送る splat の上限(storage buffer の固定長)
pub const MAX_SPLATS: usize = 1024;

/// 全シミュレーションパラメータの唯一の置き場(H2)。
/// フィールドを足したら: ①ここに1行 ②app.rs のスライダー1行 ③WGSL の struct に1行。
/// メモリレイアウトは WGSL の uniform 規則(16 バイト整列)に合わせること。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable, Serialize, Deserialize)]
pub struct SimParams {
    /// ブラシ半径(テクセル単位)
    pub brush_radius: f32,
    /// 1 splat あたりの塗り強さ(0..1)
    pub brush_flow: f32,
    #[serde(skip)]
    pub _pad: [f32; 2],
    /// ブラシ色(RGBA、ガンマ空間)
    pub brush_color: [f32; 4],
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            brush_radius: 16.0,
            brush_flow: 0.35,
            _pad: [0.0; 2],
            brush_color: [0.13, 0.30, 0.55, 1.0],
        }
    }
}

/// ストローク上の1点。WGSL 側の Splat と同レイアウト(16 バイト)。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Splat {
    /// テクセル座標
    pub pos: [f32; 2],
    /// 筆圧(マウスは 1.0。M1.5 で octotablet の値が入る)
    pub pressure: f32,
    pub _pad: f32,
}

impl Splat {
    pub fn new(pos: [f32; 2], pressure: f32) -> Self {
        Self {
            pos,
            pressure,
            _pad: 0.0,
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
