//! ストローク記録・再生の永続化(H5)。
//!
//! モデル本体(TimedPoint / RecordedStroke / Recording / Recorder / Player)は
//! CPU 純粋部として [`paint_core::replay`] にある(refactoring.md R1)。ここはそれを
//! 再エクスポートしつつ、アセットディレクトリ解決(assets.rs、CARGO_MANIFEST_DIR 基準)に
//! 依存するファイル保存/読込だけを持つ。呼び出し側は従来どおり `crate::replay::*` で使える。
//!
//! assets/strokes/*.json は git にコミットする(代表的なテストストロークの同梱)。

use crate::assets::{asset_dir, list_json_names};
use std::path::PathBuf;

pub use paint_core::replay::*;

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

    /// リポジトリ同梱のテストストロークが全部読めること(壊れた JSON のコミットを防ぐ)
    #[test]
    fn bundled_strokes_load() {
        for name in list() {
            let recording = load(&name).unwrap_or_else(|e| panic!("ストローク {name}: {e}"));
            assert!(!recording.is_empty(), "{name} が空です");
        }
    }
}
