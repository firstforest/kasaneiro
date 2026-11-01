//! ストローク記録・再生モデル(H5)。
//!
//! 記録するのは「生のポインタ入力」(刻み番号+テクセル座標+筆圧)であって
//! splat 列ではない。再生時に StrokeState の補間を通し直すため、ブラシ半径などの
//! パラメータを変えて同一ストロークを再描画できる(= A/B 比較。docs/design.md §2 の狙い)。
//! 顔料スロットだけはストロークの意味(何色で描いたか)なのでストロークごとに記録する。
//!
//! ファイルへの保存/読込(assets/strokes/*.json)はアセットディレクトリ解決に依存するため
//! バイナリ crate 側(src/replay.rs)に置く。ここはその永続化が使う純粋なモデルだけを持つ。

use crate::brush::StrokeState;
use crate::sim::{SimParams, Splat};
use serde::{Deserialize, Serialize};

/// 記録開始からの刻み番号付きポインタ位置(テクセル座標)。
/// 刻みは 1/60 秒([`crate::clock::SimClock`])。JSON のキー名は互換のため `frame` のまま
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimedPoint {
    pub frame: u32,
    pub pos: [f32; 2],
    pub pressure: f32,
}

/// 1 ストローク = ドラッグ開始〜終了。channel はそのとき選ばれていた顔料スロット、
/// tool はツール(0=描画/1=リフト/2=消去。M3)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordedStroke {
    pub channel: u32,
    pub tool: u32,
    pub points: Vec<TimedPoint>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Recording {
    pub strokes: Vec<RecordedStroke>,
}

impl Recording {
    pub fn is_empty(&self) -> bool {
        self.strokes.iter().all(|s| s.points.is_empty())
    }
}

/// 記録中の状態。1刻みごとに tick() し、ポインタイベントを積む
#[derive(Default)]
pub struct Recorder {
    recording: Recording,
    frame: u32,
    in_stroke: bool,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            recording: Recording::default(),
            frame: 0,
            in_stroke: false,
        }
    }

    /// 1刻みに1回呼ぶ(ストローク間の「待ち」も再現するため無入力でも進める)
    pub fn tick(&mut self) {
        self.frame += 1;
    }

    pub fn begin_stroke(&mut self, channel: u32, tool: u32) {
        self.recording.strokes.push(RecordedStroke {
            channel,
            tool,
            points: Vec::new(),
        });
        self.in_stroke = true;
    }

    pub fn add_point(&mut self, pos: [f32; 2], pressure: f32) {
        if !self.in_stroke {
            // ドラッグ開始イベントを取りこぼした場合の保険
            self.begin_stroke(0, 0);
        }
        if let Some(stroke) = self.recording.strokes.last_mut() {
            stroke.points.push(TimedPoint {
                frame: self.frame,
                pos,
                pressure,
            });
        }
    }

    pub fn end_stroke(&mut self) {
        self.in_stroke = false;
    }

    pub fn finish(self) -> Recording {
        self.recording
    }
}

/// 再生中の状態。1刻みごとに advance() を呼ぶと記録時と同じテンポで splat が出る
pub struct Player {
    recording: Recording,
    frame: u32,
    stroke_idx: usize,
    point_idx: usize,
    stroke_state: StrokeState,
    in_stroke: bool,
}

impl Player {
    pub fn new(recording: Recording) -> Self {
        Self {
            recording,
            frame: 0,
            stroke_idx: 0,
            point_idx: 0,
            stroke_state: StrokeState::default(),
            in_stroke: false,
        }
    }

    /// 1刻み分のポインタ入力を再生して splat を out に積む。
    /// ライブ描画と同じ経路(StrokeState → splat.wgsl)を通すため、現在のブラシ
    /// パラメータがそのまま効く。顔料スロットとツールだけは記録値で params を上書きする。
    /// UI 側が毎フレーム params.tool を選択中ツールで書き直すため、ストローク中は
    /// 毎刻み記録値を書き直す。再生し終えたら false を返す。
    pub fn advance(&mut self, params: &mut SimParams, out: &mut Vec<Splat>) -> bool {
        if self.in_stroke
            && let Some(stroke) = self.recording.strokes.get(self.stroke_idx)
        {
            params.brush_channel = stroke.channel;
            params.tool = stroke.tool;
        }
        while self.stroke_idx < self.recording.strokes.len() {
            let stroke = &self.recording.strokes[self.stroke_idx];
            // 空ストロークや、現在の刻みに達していない先頭点はここで判定
            let Some(point) = stroke.points.get(self.point_idx) else {
                // ストロークを消費し切った → 終了処理して次へ
                if self.in_stroke {
                    self.stroke_state.end();
                    self.in_stroke = false;
                }
                self.stroke_idx += 1;
                self.point_idx = 0;
                continue;
            };
            if point.frame > self.frame {
                break; // この刻みの分はおしまい(記録時のテンポを保つ)
            }
            if !self.in_stroke {
                self.stroke_state.begin();
                self.in_stroke = true;
                params.brush_channel = stroke.channel;
                // ツールも記録値で上書き(リフト/消去ストロークを正しく再現する。M3)
                params.tool = stroke.tool;
            }
            // サンプル間隔は筆圧を反映した実効半径から(ライブ描画 src/app/mod.rs と同じ式)
            let spacing = (params.radius_at(point.pressure) * 0.25).max(1.0);
            self.stroke_state
                .add_motion(point.pos, point.pressure, spacing, out);
            self.point_idx += 1;
        }
        self.frame += 1;
        self.stroke_idx < self.recording.strokes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 記録 → 再生の往復: 記録時と同じ刻みに同じ座標の splat が出ること
    #[test]
    fn record_then_replay() {
        let mut recorder = Recorder::new();
        recorder.begin_stroke(2, 1);
        recorder.add_point([10.0, 10.0], 1.0);
        recorder.tick();
        recorder.add_point([20.0, 10.0], 1.0);
        recorder.end_stroke();
        let recording = recorder.finish();

        let mut player = Player::new(recording);
        let mut params = SimParams::default();
        let mut out = Vec::new();

        // 刻み0: 始点の splat + 顔料スロット・ツールの上書き
        assert!(player.advance(&mut params, &mut out));
        assert_eq!(params.brush_channel, 2);
        assert_eq!(params.tool, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pos, [10.0, 10.0]);

        // 刻み1: 補間された splat 列。以降は入力が尽きて終了
        out.clear();
        player.advance(&mut params, &mut out);
        assert!(!out.is_empty());
        assert_eq!(out.last().unwrap().pos, [20.0, 10.0]);
        assert!(!player.advance(&mut params, &mut out));
    }

    /// UI が毎フレーム params.tool / brush_channel を書き直しても、ストローク中は
    /// 2刻み目以降も記録値で splat が出ること
    #[test]
    fn replay_keeps_recorded_tool_every_frame() {
        let mut recorder = Recorder::new();
        recorder.begin_stroke(2, 1);
        recorder.add_point([10.0, 10.0], 1.0);
        recorder.tick();
        recorder.add_point([20.0, 10.0], 1.0);
        recorder.tick();
        recorder.add_point([30.0, 10.0], 1.0);
        recorder.end_stroke();
        let mut player = Player::new(recorder.finish());
        let mut params = SimParams::default();
        let mut out = Vec::new();

        player.advance(&mut params, &mut out);
        for _ in 0..2 {
            // UI の選択ツール・顔料での上書き(tools.rs が advance より先に走る)
            params.tool = 0;
            params.brush_channel = 0;
            out.clear();
            player.advance(&mut params, &mut out);
            assert!(!out.is_empty());
            assert_eq!(params.tool, 1);
            assert_eq!(params.brush_channel, 2);
        }
    }
}
