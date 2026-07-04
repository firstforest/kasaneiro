//! SimParams のプリセット保存/読込(H3)。
//! assets/presets/*.json は git にコミットする(plan.md §1: 「昨日の良かった設定」を失わない)。
//! SimParams 側に #[serde(default)] があるため、パラメータが増えても古いプリセットは
//! 不足分を既定値で埋めて読める。

use crate::assets::{asset_dir, list_json_names};
use crate::sim::SimParams;
use std::path::PathBuf;

pub fn presets_dir() -> PathBuf {
    asset_dir("presets")
}

/// 保存済みプリセット名の一覧(ソート済み)
pub fn list() -> Vec<String> {
    list_json_names(&presets_dir())
}

pub fn save(name: &str, params: &SimParams) -> Result<PathBuf, String> {
    let dir = presets_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(params).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{} に書けません: {e}", path.display()))?;
    Ok(path)
}

pub fn load(name: &str) -> Result<SimParams, String> {
    let path = presets_dir().join(format!("{name}.json"));
    let json = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    serde_json::from_str(&json).map_err(|e| format!("{name}.json の形式が不正です: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JSON 往復で値が保たれること + フィールド欠落(古いプリセット)が既定値で埋まること
    #[test]
    fn roundtrip_and_missing_fields() {
        let params = SimParams {
            brush_radius: 32.0,
            ..Default::default()
        };
        let json = serde_json::to_string(&params).unwrap();
        let back: SimParams = serde_json::from_str(&json).unwrap();
        assert_eq!(params, back);

        let partial: SimParams = serde_json::from_str(r#"{ "brush_radius": 8.0 }"#).unwrap();
        assert_eq!(partial.brush_radius, 8.0);
        assert_eq!(partial.evap_rate, SimParams::default().evap_rate);
    }

    /// リポジトリ同梱のプリセットが全部読めること(壊れた JSON のコミットを防ぐ)
    #[test]
    fn bundled_presets_load() {
        for name in list() {
            load(&name).unwrap_or_else(|e| panic!("プリセット {name}: {e}"));
        }
    }
}
