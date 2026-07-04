//! ストローク → splat 列への変換。
//! ポインタイベントは飛び飛びに来るので、前回位置から等間隔に補間して隙間のない線にする。

use crate::sim::{MAX_SPLATS, Splat};

#[derive(Default)]
pub struct StrokeState {
    last: Option<[f32; 2]>,
}

impl StrokeState {
    pub fn begin(&mut self) {
        self.last = None;
    }

    pub fn end(&mut self) {
        self.last = None;
    }

    /// ポインタ位置(テクセル座標)を受け取り、補間した splat 列を out に積む。
    /// spacing はサンプル間隔(テクセル)。ブラシ半径の 1/4 程度が目安。
    pub fn add_motion(&mut self, pos: [f32; 2], pressure: f32, spacing: f32, out: &mut Vec<Splat>) {
        let spacing = spacing.max(0.5);
        match self.last {
            None => {
                out.push(Splat::new(pos, pressure));
                self.last = Some(pos);
            }
            Some(prev) => {
                let dx = pos[0] - prev[0];
                let dy = pos[1] - prev[1];
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < spacing {
                    // 間隔に満たない移動は溜めておき、次の点とまとめて補間する
                    return;
                }
                let steps = (dist / spacing).ceil() as usize;
                for i in 1..=steps {
                    if out.len() >= MAX_SPLATS {
                        break;
                    }
                    let t = i as f32 / steps as f32;
                    out.push(Splat::new(
                        [prev[0] + dx * t, prev[1] + dy * t],
                        pressure,
                    ));
                }
                self.last = Some(pos);
            }
        }
    }
}
