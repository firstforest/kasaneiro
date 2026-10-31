//! SimParams のプリセット保存/読込(H3)。
//! assets/presets/*.json は git にコミットする(plan.md §1: 「昨日の良かった設定」を失わない)。
//! SimParams 側に #[serde(default)] があるため、パラメータが増えても古いプリセットは
//! 不足分を既定値で埋めて読める。

use crate::assets::{asset_dir, json_list, json_load, json_save};
use paint_core::sim::SimParams;
use std::path::PathBuf;

pub fn presets_dir() -> PathBuf {
    asset_dir("presets")
}

/// 保存済みプリセット名の一覧(ソート済み)
pub fn list() -> Vec<String> {
    json_list(&presets_dir())
}

pub fn save(name: &str, params: &SimParams) -> Result<PathBuf, String> {
    json_save(&presets_dir(), name, params, true)
}

pub fn load(name: &str) -> Result<SimParams, String> {
    json_load(&presets_dir(), name)
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
