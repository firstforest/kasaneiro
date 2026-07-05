//! paint-core: GPU に触らない CPU 純粋部(refactoring.md R1)。
//!
//! シミュレーションパラメータ・splat(sim)、ストローク補間(brush)、
//! 記録再生モデル(replay)、紙ノイズ生成(paper)を持つ。依存は bytemuck + serde のみで、
//! `cargo test -p paint-core` が wgpu をリンクせず数秒で回る。
//! ストローク記録の永続化(ファイル I/O)はアセットディレクトリ解決に依存するため
//! バイナリ crate 側(src/replay.rs)に置く。

pub mod brush;
pub mod paper;
pub mod replay;
pub mod sim;
