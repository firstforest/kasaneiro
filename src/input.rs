//! 入力抽象。ポインタ入力を PointerEvent に正規化する。
//!
//! ソースは2つ(M1.5): egui ドラッグのマウス(MouseSource)と、egui の Touch
//! イベントを読むペン(PenSource)。筆圧の経路は
//! ペン → Windows Ink(WM_POINTER)→ winit が GetPointerPenInfo で筆圧を取得し
//! Touch{force} を送出 → egui-winit が egui::Event::Touch{force} に変換、で
//! **egui だけで完結する**(octotablet は Windows でメッセージループがデッドロック
//! する既知バグがあり不採用: https://github.com/Fuzzyzilla/octotablet/issues/18)。
//!
//! egui-winit は Touch から通常のポインタ(カーソル移動+左ボタン)もエミュレート
//! するため、ペン接地中は PenSource を優先し MouseSource を無視する(二重ストローク
//! 防止)。将来の入力(wasm Pointer 等)は PointerSource の実装を足す(docs/design.md §3.8)。

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
}

/// 正規化されたポインタイベント。座標は egui のポイント(ウィンドウ論理ピクセル)
#[derive(Clone, Copy, Debug)]
pub struct PointerEvent {
    pub phase: PointerPhase,
    pub pos: egui::Pos2,
    /// 筆圧 0..1。マウスは 1.0
    pub pressure: f32,
}

pub trait PointerSource {
    /// フレームごとに1回呼ぶ。このフレームのポインタイベント列を返す。
    /// canvas はキャンバス領域の Response(マウス実装がドラッグ状態を読む)
    fn poll(&mut self, canvas: &egui::Response) -> Vec<PointerEvent>;

    /// このソースが現在入力を担っているか(ペンが接地中など)。
    /// true のソースがマウスより優先される
    fn is_active(&self) -> bool;
}

/// egui のドラッグをポインタイベントに変換する(筆圧は常に 1.0)
#[derive(Default)]
pub struct MouseSource;

impl PointerSource for MouseSource {
    fn poll(&mut self, canvas: &egui::Response) -> Vec<PointerEvent> {
        let mut events = Vec::new();
        let Some(pos) = canvas.interact_pointer_pos() else {
            return events;
        };
        if canvas.drag_started() {
            events.push(PointerEvent {
                phase: PointerPhase::Down,
                pos,
                pressure: 1.0,
            });
        } else if canvas.dragged() {
            events.push(PointerEvent {
                phase: PointerPhase::Move,
                pos,
                pressure: 1.0,
            });
        }
        if canvas.drag_stopped() {
            events.push(PointerEvent {
                phase: PointerPhase::Up,
                pos,
                pressure: 1.0,
            });
        }
        events
    }

    fn is_active(&self) -> bool {
        true
    }
}

/// egui の Touch イベントからペン(筆圧付き)/タッチ入力を拾う(M1.5)。
/// 最初に接地した1本だけを追跡し、接地中の他のタッチは無視する(パーム対策を兼ねる)
#[derive(Default)]
pub struct PenSource {
    /// 追跡中のタッチ ID(接地〜離れるまで安定)
    active_id: Option<egui::TouchId>,
    /// 直近の筆圧(UI のデバッグ表示用。接地中のみ Some)
    last_pressure: Option<f32>,
    /// 直前のまとまりの生の筆圧(まとめて届いた点へ筆圧を割り振る起点。接地中のみ Some)
    bundle_prev: Option<f32>,
}

impl PenSource {
    /// 直近の筆圧(ペンが接地中のときだけ Some。UI の状態表示用)
    pub fn last_pressure(&self) -> Option<f32> {
        self.last_pressure
    }
}

impl PointerSource for PenSource {
    fn poll(&mut self, canvas: &egui::Response) -> Vec<PointerEvent> {
        let mut out = Vec::new();
        // input のロック中に layer_id_at(memory のロック)を呼ばないよう、先に取り出す
        let touches: Vec<egui::Event> = canvas.ctx.input(|input| {
            input
                .events
                .iter()
                .filter(|e| matches!(e, egui::Event::Touch { .. }))
                .cloned()
                .collect()
        });
        for event in &touches {
            let egui::Event::Touch {
                id,
                phase,
                pos,
                force,
                ..
            } = *event
            else {
                continue;
            };
            // winit(Windows)は筆圧 0 を None で送る。勢いよく払うとペンが離れる直前の位置が
            // まとめて筆圧 0 で届き、軽く触れた筆入れも 0 で届きうるので、None は常に 0 と読む
            // (1.0 と読むと筆入れ・筆抜きが最大の太さで丸く膨らむ)。筆圧を返さない指タッチは
            // 最も細い線になる(入力の対象は Windows Ink のペン)
            let tracked = self.active_id == Some(id);
            let pressure = force.unwrap_or(0.0);
            match phase {
                egui::TouchPhase::Start => {
                    // 生 Touch はウィジェットの重なりを見ないので、接地点の最前面が
                    // キャンバスのレイヤーのときだけ受け付ける(モーダル・ポップアップの裏に描かない)
                    let on_canvas = canvas.ctx.layer_id_at(pos) == Some(canvas.layer_id);
                    if self.active_id.is_none() && on_canvas {
                        self.active_id = Some(id);
                        self.last_pressure = Some(pressure);
                        out.push(PointerEvent {
                            phase: PointerPhase::Down,
                            pos,
                            pressure,
                        });
                    }
                }
                // ペンのホバー(接地前)でも Move は届くが、追跡中でなければ無視
                egui::TouchPhase::Move => {
                    if tracked {
                        self.last_pressure = Some(pressure);
                        out.push(PointerEvent {
                            phase: PointerPhase::Move,
                            pos,
                            pressure,
                        });
                    }
                }
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    if tracked {
                        self.active_id = None;
                        self.last_pressure = None;
                        out.push(PointerEvent {
                            phase: PointerPhase::Up,
                            pos,
                            pressure,
                        });
                    }
                }
            }
        }
        spread_bundles(&mut out, &mut self.bundle_prev);
        out
    }

    fn is_active(&self) -> bool {
        self.active_id.is_some()
    }
}

/// まとめて届いた点に筆圧を割り振る。winit(Windows)は1メッセージにまとまった過去の位置すべてに
/// 受け取った時点の筆圧を付けるので、1フレームに数点が同じ筆圧で届き、太さが階段状に変わる。
/// 同じ筆圧の Move の連続を1つのまとまりとみなし、直前のまとまりの筆圧からこのまとまりの筆圧へ
/// 線形に割り振る(最後の点がこのまとまりの筆圧)。`prev` は直前のまとまりの生の筆圧で、
/// Down で始まり Up で消える(poll をまたいで持ち越す)
fn spread_bundles(events: &mut [PointerEvent], prev: &mut Option<f32>) {
    let mut i = 0;
    while i < events.len() {
        match events[i].phase {
            PointerPhase::Down => {
                *prev = Some(events[i].pressure);
                i += 1;
            }
            PointerPhase::Up => {
                *prev = None;
                i += 1;
            }
            PointerPhase::Move => {
                let raw = events[i].pressure;
                let mut j = i + 1;
                while j < events.len()
                    && events[j].phase == PointerPhase::Move
                    && events[j].pressure == raw
                {
                    j += 1;
                }
                if let Some(from) = *prev {
                    let n = (j - i) as f32;
                    for (k, ev) in events[i..j].iter_mut().enumerate() {
                        ev.pressure = from + (raw - from) * (k + 1) as f32 / n;
                    }
                }
                *prev = Some(raw);
                i = j;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(phase: PointerPhase, pressure: f32) -> PointerEvent {
        PointerEvent {
            phase,
            pos: egui::Pos2::ZERO,
            pressure,
        }
    }

    fn pressures(events: &[PointerEvent]) -> Vec<f32> {
        events.iter().map(|e| e.pressure).collect()
    }

    /// 同じ筆圧でまとめて届いた点は、直前の筆圧からこの筆圧へ線形に割り振られる
    #[test]
    fn bundle_ramps_from_previous_pressure() {
        let mut prev = None;
        let mut a = vec![ev(PointerPhase::Down, 0.2), ev(PointerPhase::Move, 0.2)];
        spread_bundles(&mut a, &mut prev);
        assert_eq!(pressures(&a), vec![0.2, 0.2]);
        // 次のフレーム: 4点が 0.6 で届く → 0.3, 0.4, 0.5, 0.6
        let mut b = vec![ev(PointerPhase::Move, 0.6); 4];
        spread_bundles(&mut b, &mut prev);
        for (got, want) in pressures(&b).iter().zip([0.3, 0.4, 0.5, 0.6]) {
            assert!((got - want).abs() < 1e-6, "{:?}", pressures(&b));
        }
        // 筆圧の違う2つのまとまりが同じフレームに来たら別々に割り振る
        let mut c = vec![
            ev(PointerPhase::Move, 0.4),
            ev(PointerPhase::Move, 0.4),
            ev(PointerPhase::Move, 0.0),
            ev(PointerPhase::Move, 0.0),
            ev(PointerPhase::Up, 0.0),
        ];
        spread_bundles(&mut c, &mut prev);
        for (got, want) in pressures(&c).iter().zip([0.5, 0.4, 0.2, 0.0, 0.0]) {
            assert!((got - want).abs() < 1e-6, "{:?}", pressures(&c));
        }
        assert_eq!(prev, None);
    }

    /// 筆圧が変わらないまま届く点はそのまま
    #[test]
    fn steady_pressure_is_unchanged() {
        let mut prev = None;
        let mut a = vec![
            ev(PointerPhase::Down, 0.5),
            ev(PointerPhase::Move, 0.5),
            ev(PointerPhase::Move, 0.5),
        ];
        spread_bundles(&mut a, &mut prev);
        assert_eq!(pressures(&a), vec![0.5, 0.5, 0.5]);
    }
}
