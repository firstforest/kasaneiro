//! 入力抽象(plan.md §2 の input.rs)。ポインタ入力を PointerEvent に正規化する。
//!
//! ソースは2つ(M1.5): egui 経由のマウス(MouseSource)と octotablet 経由の
//! ペン(TabletSource、Windows Ink。筆圧が取れる)。ペンが検知範囲内にいる間は
//! TabletSource を優先する — Windows Ink のペンは OS がマウスカーソルも動かすため、
//! 両方を処理すると二重ストロークになる。将来の入力(wasm Pointer 等)は
//! PointerSource の実装を足す(plan.md §4: trait 抽象だけ維持)。

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
}

/// 正規化されたポインタイベント。
/// 座標はウィンドウ左上基準の論理ピクセル(egui のポイントと同じ空間。
/// octotablet の Pose.position も同じ空間で届く)。
#[derive(Clone, Copy, Debug)]
pub struct PointerEvent {
    pub phase: PointerPhase,
    pub pos: egui::Pos2,
    /// 筆圧 0..1。筆圧のないソース(マウス)は常に 1.0
    pub pressure: f32,
}

pub trait PointerSource {
    /// フレームごとに1回呼ぶ。このフレームのポインタイベント列を返す。
    /// canvas はキャンバス領域の Response(マウス実装がドラッグ状態を読む)
    fn poll(&mut self, canvas: &egui::Response) -> Vec<PointerEvent>;

    /// このソースが現在入力を担っているか(ペンが検知範囲内など)。
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

/// octotablet(Windows Ink)のペン入力(M1.5)。
/// 接続に失敗してもアプリは動く(マウスへフォールバック。状態は error() で UI に出す)
pub struct TabletSource {
    /// Err = 接続失敗の理由(UI 表示用)。disconnect() 後も Err になる
    manager: Result<octotablet::Manager, String>,
    /// ペンが検知範囲内(In〜Out の間)
    in_range: bool,
    /// ペンが接地中(Down〜Up の間)
    down: bool,
    /// 直近の Pose 位置(Down/Up イベント自体は座標を持たないため保持する)
    last_pos: egui::Pos2,
    /// 直近の筆圧(ホバー中は 0 付近。UI のデバッグ表示にも使う)
    last_pressure: f32,
}

impl TabletSource {
    /// eframe の CreationContext(HasWindowHandle + HasDisplayHandle)から接続する。
    ///
    /// build_raw の安全条件: ウィンドウ・ディスプレイのハンドルが Manager より
    /// 長生きすること。Manager は PaintApp が保持し、ウィンドウ破棄前の
    /// on_exit で disconnect() するので満たされる。
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let manager = unsafe {
            octotablet::builder::Builder::new()
                // マウスをツール扱いしない: マウスは egui 経由の MouseSource が担当
                .emulate_tool_from_mouse(false)
                .build_raw(cc)
        }
        .map_err(|e| e.to_string());
        if let Err(e) = &manager {
            log::warn!("タブレット API に接続できません(マウスのみで続行): {e}");
        }
        Self {
            manager,
            in_range: false,
            down: false,
            last_pos: egui::Pos2::ZERO,
            last_pressure: 0.0,
        }
    }

    /// 接続失敗の理由(接続できていれば None)
    pub fn error(&self) -> Option<&str> {
        self.manager.as_ref().err().map(String::as_str)
    }

    /// 直近の筆圧(ペンが検知範囲内のときだけ Some)
    pub fn last_pressure(&self) -> Option<f32> {
        self.in_range.then_some(self.last_pressure)
    }

    /// アプリ終了時に呼ぶ。ウィンドウ破棄前に接続を切る(build_raw の安全条件)
    pub fn disconnect(&mut self) {
        self.manager = Err("切断済み".to_owned());
        self.in_range = false;
        self.down = false;
    }
}

impl PointerSource for TabletSource {
    fn poll(&mut self, _canvas: &egui::Response) -> Vec<PointerEvent> {
        let mut out = Vec::new();
        let Ok(manager) = &mut self.manager else {
            return out;
        };
        // Windows(Ink)では PumpError は発生しない(空 enum)が、
        // 将来 Wayland 等でビルドしても動くよう match で処理する
        let events = match manager.pump() {
            Ok(events) => events,
            Err(e) => {
                log::warn!("タブレットイベントの取得に失敗: {e}");
                return out;
            }
        };
        for event in events {
            use octotablet::events::{Event, ToolEvent};
            let Event::Tool { event, .. } = event else {
                continue;
            };
            match event {
                ToolEvent::In { .. } => self.in_range = true,
                ToolEvent::Pose(pose) => {
                    self.last_pos = egui::pos2(pose.position[0], pose.position[1]);
                    if let Some(p) = pose.pressure.get() {
                        self.last_pressure = p;
                    }
                    if self.down {
                        out.push(PointerEvent {
                            phase: PointerPhase::Move,
                            pos: self.last_pos,
                            pressure: self.last_pressure,
                        });
                    }
                }
                ToolEvent::Down => {
                    self.down = true;
                    out.push(PointerEvent {
                        phase: PointerPhase::Down,
                        pos: self.last_pos,
                        pressure: self.last_pressure,
                    });
                }
                ToolEvent::Up => {
                    if self.down {
                        out.push(PointerEvent {
                            phase: PointerPhase::Up,
                            pos: self.last_pos,
                            pressure: self.last_pressure,
                        });
                    }
                    self.down = false;
                }
                ToolEvent::Out | ToolEvent::Removed => {
                    // 接地したまま検知範囲を抜けた場合もストロークを閉じる
                    if self.down {
                        out.push(PointerEvent {
                            phase: PointerPhase::Up,
                            pos: self.last_pos,
                            pressure: self.last_pressure,
                        });
                        self.down = false;
                    }
                    self.in_range = false;
                }
                _ => {}
            }
        }
        out
    }

    fn is_active(&self) -> bool {
        self.in_range
    }
}
