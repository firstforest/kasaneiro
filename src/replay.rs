//! ストローク記録・再生(H5)。
//!
//! 記録するのは「生のポインタ入力」(フレーム番号+テクセル座標+筆圧)であって
//! splat 列ではない。再生時に StrokeState の補間を通し直すため、ブラシ半径などの
//! パラメータを変えて同一ストロークを再描画できる(= A/B 比較。plan.md §1 の狙い)。
//! 顔料スロットだけはストロークの意味(何色で描いたか)なのでストロークごとに記録する。
//!
//! assets/strokes/*.json は git にコミットする(代表的なテストストロークの同梱)。

use crate::assets::{asset_dir, list_json_names};
use crate::brush::StrokeState;
use crate::sim::{SimParams, Splat};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 記録開始からのフレーム番号付きポインタ位置(テクセル座標)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimedPoint {
    pub frame: u32,
    pub pos: [f32; 2],
    pub pressure: f32,
}

/// 1 ストローク = ドラッグ開始〜終了。channel はそのとき選ばれていた顔料スロット
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordedStroke {
    pub channel: u32,
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

/// 記録中の状態。アプリのフレームごとに tick() し、ポインタイベントを積む
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

    /// 毎フレーム1回呼ぶ(ストローク間の「待ち」も再現するため無入力でも進める)
    pub fn tick(&mut self) {
        self.frame += 1;
    }

    pub fn begin_stroke(&mut self, channel: u32) {
        self.recording.strokes.push(RecordedStroke {
            channel,
            points: Vec::new(),
        });
        self.in_stroke = true;
    }

    pub fn add_point(&mut self, pos: [f32; 2], pressure: f32) {
        if !self.in_stroke {
            // ドラッグ開始イベントを取りこぼした場合の保険
            self.begin_stroke(0);
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

/// 再生中の状態。毎フレーム advance() を呼ぶと記録時と同じテンポで splat が出る
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

    /// 1フレーム分のポインタ入力を再生して splat を out に積む。
    /// ライブ描画と同じ経路(StrokeState → splat.wgsl)を通すため、現在のブラシ
    /// パラメータがそのまま効く。顔料スロットだけは記録値で params を上書きする。
    /// 再生し終えたら false を返す。
    pub fn advance(&mut self, params: &mut SimParams, out: &mut Vec<Splat>) -> bool {
        while self.stroke_idx < self.recording.strokes.len() {
            let stroke = &self.recording.strokes[self.stroke_idx];
            // 空ストロークや、現フレームに達していない先頭点はここで判定
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
                break; // このフレームの分はおしまい(記録時のテンポを保つ)
            }
            if !self.in_stroke {
                self.stroke_state.begin();
                self.in_stroke = true;
                params.brush_channel = stroke.channel;
            }
            let spacing = (params.brush_radius * 0.25).max(1.0);
            self.stroke_state
                .add_motion(point.pos, point.pressure, spacing, out);
            self.point_idx += 1;
        }
        self.frame += 1;
        self.stroke_idx < self.recording.strokes.len()
    }
}

pub fn strokes_dir() -> PathBuf {
    asset_dir("strokes")
}

/// 保存済みストローク名の一覧(ソート済み)
pub fn list() -> Vec<String> {
    list_json_names(&strokes_dir())
}

pub fn save(name: &str, recording: &Recording) -> Result<PathBuf, String> {
    let dir = strokes_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.json"));
    let json = serde_json::to_string(recording).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{} に書けません: {e}", path.display()))?;
    Ok(path)
}

pub fn load(name: &str) -> Result<Recording, String> {
    let path = strokes_dir().join(format!("{name}.json"));
    let json = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    serde_json::from_str(&json).map_err(|e| format!("{name}.json の形式が不正です: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 記録 → 再生の往復: 記録時と同じフレームに同じ座標の splat が出ること
    #[test]
    fn record_then_replay() {
        let mut recorder = Recorder::new();
        recorder.begin_stroke(2);
        recorder.add_point([10.0, 10.0], 1.0);
        recorder.tick();
        recorder.add_point([20.0, 10.0], 1.0);
        recorder.end_stroke();
        let recording = recorder.finish();

        let mut player = Player::new(recording);
        let mut params = SimParams::default();
        let mut out = Vec::new();

        // フレーム0: 始点の splat + 顔料スロットの上書き
        assert!(player.advance(&mut params, &mut out));
        assert_eq!(params.brush_channel, 2);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pos, [10.0, 10.0]);

        // フレーム1: 補間された splat 列。以降は入力が尽きて終了
        out.clear();
        player.advance(&mut params, &mut out);
        assert!(!out.is_empty());
        assert_eq!(out.last().unwrap().pos, [20.0, 10.0]);
        assert!(!player.advance(&mut params, &mut out));
    }

    /// リポジトリ同梱のテストストロークが全部読めること
    #[test]
    fn bundled_strokes_load() {
        for name in list() {
            let recording = load(&name).unwrap_or_else(|e| panic!("ストローク {name}: {e}"));
            assert!(!recording.is_empty(), "{name} が空です");
        }
    }
}
