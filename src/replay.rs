//! ストローク記録・再生の永続化(H5)。
//!
//! モデル本体(TimedPoint / RecordedStroke / Recording / Recorder / Player)は
//! CPU 純粋部として [`paint_core::replay`] にある(refactoring.md R1)。ここはそれを
//! 再エクスポートしつつ、アセットディレクトリ解決(assets.rs、CARGO_MANIFEST_DIR 基準)に
//! 依存するファイル保存/読込だけを持つ。呼び出し側は従来どおり `crate::replay::*` で使える。
//!
//! assets/strokes/*.json は git にコミットする(代表的なテストストロークの同梱)。

use crate::assets::{asset_dir, list_json_names};
use pigment::Palette;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use paint_core::replay::*;

/// ファイルに保存するストローク記録(M5d)。生ポインタ入力([`Recording`])に、記録時の
/// パレット(顔料の色・個性)を添える。再生時にこのパレットへ切り替えると、後から顔料を
/// 編集しても記録が「当時の色」で再生される(スロット番号だけでは色が変わり A/B が壊れる)。
///
/// `#[serde(flatten)]` で `Recording` の `strokes` を直下に展開するので、JSON は
/// `{"strokes":[...],"palette":{...}}`。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredRecording {
    #[serde(flatten)]
    pub recording: Recording,
    /// 記録時のパレット(必須。再生は常にこのパレットへ切り替える)
    pub palette: Palette,
}

pub fn strokes_dir() -> PathBuf {
    asset_dir("strokes")
}

/// 保存済みストローク名の一覧(ソート済み)
pub fn list() -> Vec<String> {
    list_json_names(&strokes_dir())
}

pub fn save(name: &str, recording: &Recording, palette: &Palette) -> Result<PathBuf, String> {
    let dir = strokes_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.json"));
    let stored = StoredRecording {
        recording: recording.clone(),
        palette: palette.clone(),
    };
    let json = serde_json::to_string(&stored).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{} に書けません: {e}", path.display()))?;
    Ok(path)
}

pub fn load(name: &str) -> Result<StoredRecording, String> {
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
            let stored = load(&name).unwrap_or_else(|e| panic!("ストローク {name}: {e}"));
            assert!(!stored.recording.is_empty(), "{name} が空です");
        }
    }

    /// M5d: パレット付きで保存 → 読込でパレットが復元されること
    #[test]
    fn stored_recording_roundtrip_with_palette() {
        let mut rec = Recording::default();
        rec.strokes.push(RecordedStroke {
            channel: 1,
            tool: 0,
            points: vec![TimedPoint { frame: 0, pos: [1.0, 2.0], pressure: 1.0 }],
        });
        let pal = pigment::Palette::default_palette();
        let stored = StoredRecording { recording: rec, palette: pal.clone() };
        let json = serde_json::to_string(&stored).unwrap();
        let back: StoredRecording = serde_json::from_str(&json).unwrap();
        assert_eq!(back.palette, pal);
        assert_eq!(back.recording.strokes.len(), 1);
    }
}
