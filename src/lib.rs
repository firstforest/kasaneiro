//! かさねいろ本体のライブラリ部。バイナリ(main.rs)はウィンドウを開いて [`app::PaintApp`] を
//! 走らせるだけで、e2e テスト(tests/)は同じモジュールをウィンドウなしで駆動する
//! ([`headless`] = GPU シミュ単体、`egui_kittest` = UI 込みのアプリ全体)。
//!
//! CPU 純粋部(sim / brush / paper / replay モデル)は paint-core crate、
//! 顔料・混色は pigment crate、Kubelka-Munk 参照実装は km crate にある(refactoring.md R1)。

pub mod app;
pub mod assets;
pub mod gpu;
pub mod headless;
mod input;
mod palette_store;
mod pigment_store;
mod preset;
pub mod replay;
mod work;
