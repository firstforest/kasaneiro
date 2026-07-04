//! アセットディレクトリの解決。shaders(H1)・presets(H3)・strokes(H5)が共用する。

use std::path::{Path, PathBuf};

/// assets/ 配下のサブディレクトリを返す。
/// 開発中はどこから起動しても効くように CARGO_MANIFEST_DIR 基準。
/// 存在しなければカレントディレクトリ相対にフォールバック(presets 等は保存時に作られる)。
pub fn asset_dir(sub: &str) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join(sub);
    if manifest.parent().is_some_and(|p| p.is_dir()) {
        manifest
    } else {
        PathBuf::from("assets").join(sub)
    }
}

/// dir 直下の .json のファイル名(拡張子なし)をソートして返す。
/// プリセット(H3)・ストローク(H5)の一覧 UI 用。ディレクトリが無ければ空。
pub fn list_json_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                Some(path.file_stem()?.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}
