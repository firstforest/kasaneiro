//! 入力抽象(plan.md §2 の input.rs)。ポインタ入力を PointerEvent に正規化する。
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
//! 防止)。将来の入力(wasm Pointer 等)は PointerSource の実装を足す(plan.md §4)。

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
    /// 筆圧 0..1。筆圧のないソース(マウス・筆圧非対応タッチ)は 1.0
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
        canvas.ctx.input(|input| {
            for event in &input.events {
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
                // 筆圧非対応(指タッチ等)は 1.0 = マウスと同じ扱い
                let pressure = force.unwrap_or(1.0);
                match phase {
                    egui::TouchPhase::Start => {
                        if self.active_id.is_none() {
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
                        if self.active_id == Some(id) {
                            self.last_pressure = Some(pressure);
                            out.push(PointerEvent {
                                phase: PointerPhase::Move,
                                pos,
                                pressure,
                            });
                        }
                    }
                    egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                        if self.active_id == Some(id) {
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
        });
        out
    }

    fn is_active(&self) -> bool {
        self.active_id.is_some()
    }
}
