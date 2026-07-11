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
    /// 水を置き、ブラシ下の沈着顔料を溶かし戻してぼかす(M4 → 2026-07-09 に一次原理化。
    /// 均しの箱ぼかしは廃止し、馴染ませは毛細管拡散+γ重み顔料拡散の物理に任せる)
    WaterBrush,
    /// 乾いた筆(thirsty brush): 表面の自由水と、水に浮いている顔料を吸い取る。
    /// 沈着顔料は紙に付いているので残る(剥がすのはリフト=削り)。ぼかし筆の逆方向の水管理ツール
    Absorb,
}

impl WetTool {
    /// UI・記録が回すための全列挙(順序 = ツールバーの並び)
    pub const ALL: [WetTool; 5] = [
        WetTool::Paint,
        WetTool::Lift,
        WetTool::Erase,
        WetTool::WaterBrush,
        WetTool::Absorb,
    ];

    /// `SimParams::tool` へ書く値。gpu_id を持つのは WetTool だけ —
    /// raster ツールを splat.wgsl へ流す誤りを型レベルで排除する。
    pub fn gpu_id(self) -> u32 {
        match self {
            WetTool::Paint => 0,
            WetTool::Lift => 1,
            WetTool::Erase => 2,
            WetTool::WaterBrush => 3,
            // 4 は旧ならし(2026-07-09 廃止)の跡地を再利用(リリース前につき互換考慮なし)
            WetTool::Absorb => 4,
        }
    }

    /// GPU 値 → enum。記録(replay)は on-disk では互換のため u32 のままなので、
    /// 読み戻し時にここで変換する(不正値は None)。
    pub fn from_gpu_id(id: u32) -> Option<WetTool> {
        WetTool::ALL.into_iter().find(|t| t.gpu_id() == id)
    }

    /// 選択中ツールの常時1行表示(F16)用の短い説明。左パネル幅で折り返さないよう
    /// 全角 25〜30 字以内に収める。詳しい説明はツールボタンのホバー([`ToolInfo::hint`])に温存
    pub fn short_hint(self) -> &'static str {
        match self {
            WetTool::Paint => "水と顔料を置いて描く基本のブラシ",
            WetTool::Lift => "乾いた色を水で戻して薄くする削りツール",
            WetTool::Erase => "水・顔料を消して紙の白まで戻す完全消去",
            WetTool::WaterBrush => "水だけを塗り、下の色を溶かしてぼかす",
            WetTool::Absorb => "乾いた筆で浮いた色水を吸い取る",
        }
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
        // 通常ユーザー向けに動詞・効果を主にした平易な名称(専門語は hint 側に温存)
        match self {
            WetTool::Paint => "塗る",
            WetTool::Lift => "削り",
            WetTool::Erase => "消す",
            WetTool::WaterBrush => "ぼかし筆",
            WetTool::Absorb => "吸い取り",
        }
    }

    fn hint(&self) -> &'static str {
        // 冒頭に一言で効果、続けて仕組み(専門語は末尾)。選択中ツールの説明として常時表示もされる
        match self {
            WetTool::Paint => "水と顔料を置いて描く基本のブラシ。",
            WetTool::Lift => {
                "乾いた色を水で戻して薄くする削りツール。再湿潤して沈着顔料を浮遊層へ戻す(リフト)。染みつきの強い顔料は残り、紙の凸部から先に剥がれる"
            }
            WetTool::Erase => "水・顔料をその場で消して紙の白まで戻す完全消去。",
            WetTool::WaterBrush => {
                "色を置かずに水だけ塗り、下の沈着した色を溶かして浮かせる。浮いた色は水の量に応じてひとりでに混ざって馴染む。①広い領域を先に濡らす ②境界をなでてぼかす(なでても濃くならない。ぼかしの強さ=溶かす量。染みつきの強い顔料は残る)"
            }
            WetTool::Absorb => {
                "乾いた筆やスポンジのように、まだ乾いていない水と浮いている色を吸い取る。強く押すほど・なでるほど多く吸う。紙に沈着した色は残る(剥がしたいときは「削り」)。①にじみすぎた水たまりを回収 ②ハイライトの白抜き ③水際のエッジを整える"
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
            RasterTool::Pencil => "下書き用のグレー粒状線(紙目で濃度変調、筆圧→濃さ)。",
            RasterTool::Pen => "清書用の濃色スムーズ線(筆圧→太さ)。水の境界にもなる。",
            RasterTool::Highlight => "不透明な白ブラシ(流体なし、最上段。筆圧→不透明度)。",
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
        // 4 = 旧ならし(2026-07-09 廃止)の跡地を吸い取りが再利用している
        assert_eq!(WetTool::from_gpu_id(4), Some(WetTool::Absorb));
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
