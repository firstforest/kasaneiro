//! ツールの階層 enum(refactoring.md R2)。
//!
//! **トップレベルの分岐 = 描画経路の分岐**で、型が経路を保証する:
//! - [`Tool::Wet`] は流体シミュ経由(splat バッファ → splat.wgsl)。GPU へ渡す値
//!   [`SimParams::tool`](crate::sim::SimParams::tool) は [`WetTool::gpu_id`] だけが持つ。
//! - [`Tool::Raster`] は線画テクスチャへの直描き(M4.5。流体を通らない)。GPU の
//!   splat 経路に流れないことが型レベルで保証される。
//!
//! enum(直和型)= 閉じた集合の分岐、trait([`ToolInfo`])= UI 表示の共通化、と役割を分ける。
//! ツール追加時の処理漏れは `match` の網羅性チェックがコンパイルエラーで拾う。
//!
//! ラベルは `&'static str` なので egui 依存はなく、UI 非依存の paint-core に置ける。

/// ツール全体。トップレベルの分岐がそのまま描画経路の分岐になる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// 流体シミュ経由(splat バッファ → splat.wgsl)
    Wet(WetTool),
    /// 線画テクスチャ直描き(M4.5。流体を通らない)。`eraser` はトグルで splat を減算に反転
    Raster { kind: RasterTool, eraser: bool },
}

impl Tool {
    /// 流体ツールなら中身を返す。ラスタツールは None(流体経路に流さない)。
    /// GPU へ渡す `SimParams::tool` の算出や、湿レイヤーへの splat 判定に使う。
    pub fn wet(self) -> Option<WetTool> {
        match self {
            Tool::Wet(w) => Some(w),
            Tool::Raster { .. } => None,
        }
    }
}

/// 流体シミュを通るブラシ(M1〜M4)。値 [`Self::gpu_id`] は splat.wgsl の分岐と対応。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WetTool {
    /// 水+顔料を置く(M1)
    Paint,
    /// 再湿潤して沈着顔料を浮遊層へ戻す(削り。M3)
    Lift,
    /// 湿レイヤーの水・顔料をゼロへ(完全消去。M3)
    Erase,
    /// 水を置きブラシ下の顔料を近傍平均へ均す(局所ならし。M4)
    WaterBrush,
    /// 総顔料をブラシスケールで均す(広い均一化。M4)
    Smear,
}

impl WetTool {
    /// UI・記録が回すための全列挙(順序 = ツールバーの並び)
    pub const ALL: [WetTool; 5] = [
        WetTool::Paint,
        WetTool::Lift,
        WetTool::Erase,
        WetTool::WaterBrush,
        WetTool::Smear,
    ];

    /// `SimParams::tool` へ書く値。gpu_id を持つのは WetTool だけ —
    /// raster ツールを splat.wgsl へ流す誤りを型レベルで排除する。
    pub fn gpu_id(self) -> u32 {
        match self {
            WetTool::Paint => 0,
            WetTool::Lift => 1,
            WetTool::Erase => 2,
            WetTool::WaterBrush => 3,
            WetTool::Smear => 4,
        }
    }

    /// GPU 値 → enum。記録(replay)は on-disk では互換のため u32 のままなので、
    /// 読み戻し時にここで変換する(不正値は None)。
    pub fn from_gpu_id(id: u32) -> Option<WetTool> {
        WetTool::ALL.into_iter().find(|t| t.gpu_id() == id)
    }
}

/// GPU 値(u32)からの変換。replay の `RecordedStroke.tool` など on-disk 互換のため u32 を残す箇所で使う。
impl TryFrom<u32> for WetTool {
    type Error = u32;
    fn try_from(id: u32) -> Result<Self, Self::Error> {
        WetTool::from_gpu_id(id).ok_or(id)
    }
}

/// 線画テクスチャへ直接描くツール(M4.5。流体を通らないので gpu_id を持たない)。
// M4.5a で実装・UI 追加する。型の階層だけ先に用意しておく(R2)。
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RasterTool {
    /// 下書き鉛筆(グレー粒状線、紙目で濃度変調)
    Pencil,
    /// 清書ペン(濃色スムーズ線、水の境界にもなる)
    Pen,
    /// 白ハイライト(不透明、最上段、流体なし)
    Highlight,
}

#[allow(dead_code)]
impl RasterTool {
    pub const ALL: [RasterTool; 3] = [RasterTool::Pencil, RasterTool::Pen, RasterTool::Highlight];
}

/// UI 表示用メタ情報。WetTool / RasterTool 両方に実装し、ツールバー描画は
/// ツール群を回すだけの共通コードにする(TOOLS 定数表を enum の impl に一元化する)。
pub trait ToolInfo {
    /// ボタンの短いラベル(「描画」など)
    fn label(&self) -> &'static str;
    /// ホバーで出す説明文
    fn hint(&self) -> &'static str;
}

impl ToolInfo for WetTool {
    fn label(&self) -> &'static str {
        match self {
            WetTool::Paint => "描画",
            WetTool::Lift => "リフト(削り)",
            WetTool::Erase => "消去",
            WetTool::WaterBrush => "水筆",
            WetTool::Smear => "ならし",
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            WetTool::Paint => "水+顔料を置く(M1)",
            WetTool::Lift => {
                "再湿潤して沈着顔料を浮遊層へ戻す。ステイニング顔料(ω)は残り、紙の凸部から先に剥がれる(M3)"
            }
            WetTool::Erase => "湿レイヤーの水・顔料を機械的にゼロへ(紙の白まで戻す完全消去。M3)",
            WetTool::WaterBrush => {
                "水を置き、ブラシ下の顔料を近傍平均へ均す(顔料は注入しない)。①大きな領域を先に濡らして顔料筆を滑らかに広げる ②境界をなでて均一な塗りに馴染ませる(質量保存の均しなので濃くならない。均し強度で調整)(M4)"
            }
            WetTool::Smear => {
                "濃くなった箇所に置くと、その顔料が周囲へ拡散して領域が均一に伸びていく。総顔料をブラシスケールで近傍平均へ均す(質量保存なので濃くならない)。水筆より広い範囲の均一化に。ならし強度で調整(M4)"
            }
        }
    }
}

#[allow(dead_code)]
impl ToolInfo for RasterTool {
    fn label(&self) -> &'static str {
        match self {
            RasterTool::Pencil => "鉛筆",
            RasterTool::Pen => "ペン",
            RasterTool::Highlight => "ハイライト",
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            RasterTool::Pencil => "下書き用のグレー粒状線(紙目で濃度変調、筆圧→濃さ)。M4.5",
            RasterTool::Pen => "清書用の濃色スムーズ線(筆圧→太さ)。水の境界にもなる。M4.5",
            RasterTool::Highlight => "不透明な白ブラシ(流体なし、最上段。筆圧→不透明度)。M4.5",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// gpu_id ⇄ WetTool の往復(replay の on-disk u32 互換の土台)
    #[test]
    fn gpu_id_roundtrip() {
        for t in WetTool::ALL {
            assert_eq!(WetTool::from_gpu_id(t.gpu_id()), Some(t));
            assert_eq!(WetTool::try_from(t.gpu_id()), Ok(t));
        }
        assert_eq!(WetTool::from_gpu_id(99), None);
        assert_eq!(WetTool::try_from(99u32), Err(99));
    }

    /// ラスタツールは流体経路に流れない(wet() が None)= 型が経路を保証する
    #[test]
    fn raster_is_not_wet() {
        assert_eq!(Tool::Wet(WetTool::Paint).wet(), Some(WetTool::Paint));
        assert_eq!(
            Tool::Raster { kind: RasterTool::Pen, eraser: false }.wet(),
            None
        );
    }
}
