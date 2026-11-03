//! 線画(鉛筆・ペン・ハイライト)の多段 Undo/Redo 履歴(M4.5d)。
//!
//! ラスタ線画は流体シミュを通らないので、**ストロークを決定論的に再ラスタライズ**できる:
//! Undo = 対象テクスチャをクリアして残りのストロークを引き直す / Redo = 取り消し分を再適用。
//! そのため記録するのは splat 列でなく**補間前の生ポインタ点+描画時の実効 SimParams**。
//! スライダー(半径・筆圧マッピング)を後で変えても、過去の線は保存済みパラメータで
//! 引き直されるので形が変わらない(H5 の「現在のパラメータで再生」とは目的が逆)。
//!
//! 履歴は鉛筆/ペン/ハイライトを1本の時系列で持つ。Undo で消えるのは末尾の1本で、
//! その target のテクスチャだけをクリア→再ラスタライズする(他の線種は無傷)。

use crate::gpu::LineTarget;
use paint_core::brush::{LineSmoother, StrokeState};
use paint_core::sim::{SimParams, Splat};

/// ラスタ線画の1ストローク。再ラスタライズのため生ポインタ点+実効パラメータを持つ。
#[derive(Clone)]
pub struct RasterStroke {
    /// 描画先(鉛筆 / ペン / ハイライト)。消しゴムも同じ target(減算として引き直す)
    pub target: LineTarget,
    /// 描画時点の実効 SimParams のスナップショット。再ラスタライズはこの値で linesplat を回す
    /// (linesplat が読む line_mode / line_eraser / 各半径・濃さ / 筆圧マッピングを保存する)
    pub params: SimParams,
    /// 生ポインタ点(テクセル座標, 筆圧)。再生時に StrokeState で補間し直す
    pub points: Vec<([f32; 2], f32)>,
}

/// 線画の多段 Undo/Redo 履歴。鉛筆/ペン/ハイライト共通の1本の時系列。
#[derive(Default)]
pub struct LineHistory {
    /// 適用済みストローク(古い順)
    pub done: Vec<RasterStroke>,
    /// Undo で取り消した分(新しい Redo が先頭に来るよう push/pop で末尾を使う)
    pub redo: Vec<RasterStroke>,
    /// 描画中のストローク(Down〜Up の間、生ポインタ点を溜める)
    building: Option<RasterStroke>,
}

impl LineHistory {
    /// ストローク開始(Down)。描画時の実効パラメータをスナップショットする
    pub fn begin(&mut self, target: LineTarget, params: SimParams) {
        self.building = Some(RasterStroke {
            target,
            params,
            points: Vec::new(),
        });
    }

    /// 描画中の生ポインタ点を1つ足す(補間前。テクセル座標+筆圧)
    pub fn push_point(&mut self, pos: [f32; 2], pressure: f32) {
        if let Some(b) = &mut self.building {
            b.points.push((pos, pressure));
        }
    }

    /// ストローク確定(Up)。点があれば履歴へ積み、Redo 履歴を破棄する。
    /// 確定したストロークはすでにテクスチャへライブ描画済み(再ラスタライズ不要)。
    /// 実際に1本積んだら true(統一 undo スタックへ Line マーカーを積むかの判断に使う。M6)
    pub fn finish(&mut self) -> bool {
        if let Some(b) = self.building.take()
            && !b.points.is_empty()
        {
            self.done.push(b);
            self.redo.clear();
            return true;
        }
        false
    }

    /// キャンバスリセット時に履歴も全消去する
    pub fn clear(&mut self) {
        self.done.clear();
        self.redo.clear();
        self.building = None;
    }
}

/// 線画ストロークの手ぶれ補正。清書ペンだけに掛ける(下書き鉛筆・ハイライトは入力点をそのまま使う)。
/// ライブ描画と引き直しの両方がこれで作るので、同じ生ポインタ点から同じ線になる
pub fn line_smoother(target: LineTarget, params: &SimParams) -> Option<LineSmoother> {
    (target == LineTarget::Pen).then(|| LineSmoother::new(params.pen_smooth))
}

/// ストロークの生ポインタ点を保存済みパラメータで補間し、linesplat へ渡す splat 列にする。
/// ライブ描画(app::apply_pointer_events)と同じ手ぶれ補正・間隔(実効半径×0.25)で引き直すので、
/// max 蓄積の描画は完全一致、減算(消しゴム)も概ね一致する。
pub fn stroke_splats(stroke: &RasterStroke) -> Vec<Splat> {
    let base = match stroke.target {
        LineTarget::Pencil => stroke.params.pencil_radius,
        LineTarget::Pen => stroke.params.pen_radius,
        LineTarget::Highlight => stroke.params.highlight_radius,
    };
    let spacing = |p| stroke.params.splat_spacing(base, p);
    let mut smoother = line_smoother(stroke.target, &stroke.params);
    let mut state = StrokeState::default();
    state.begin();
    let mut out = Vec::new();
    let mut points = Vec::new();
    for &(pos, pressure) in &stroke.points {
        // ライブ描画と同じ補間。GPU へ流すときは rasterize_line が MAX_SPLATS 単位に分割する
        match &mut smoother {
            Some(smoother) => smoother.push(pos, pressure, &mut points),
            None => points.push((pos, pressure)),
        }
        for (pos, pressure) in points.drain(..) {
            state.add_motion(pos, pressure, spacing, &mut out);
        }
    }
    if let Some(smoother) = &mut smoother {
        smoother.finish(&mut points);
        for (pos, pressure) in points.drain(..) {
            state.add_motion(pos, pressure, spacing, &mut out);
        }
    }
    out
}
