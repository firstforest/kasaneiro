//! パレット・ライブラリの保存/読込(M5d)。
//!
//! ランタイム編集した顔料パレット([`pigment::Palette`])を JSON で `assets/palettes/` に
//! 保存・読込する。プリセット(H3、[`crate::preset`])・ストローク(H5、[`crate::replay`])と
//! 同じ流儀(アセットディレクトリ解決 = CARGO_MANIFEST_DIR 基準、`list_json_names` で一覧)。
//! **assets/palettes/*.json は git にコミットする**(「昨日作った顔料」を失わないため)。
//!
//! Pigment のフィールドは `#[serde(default)]` ではなく素の derive だが、顔料の意味的な最小単位は
//! 4スロットまとめてなので、欠落フィールドのある壊れた JSON は素直に読込エラーにする(同梱
//! パレットが全部読めることは cargo test で担保)。

use crate::assets::{asset_dir, list_json_names};
use pigment::Palette;
use std::path::PathBuf;

pub fn palettes_dir() -> PathBuf {
    asset_dir("palettes")
}

/// 保存済みパレット名の一覧(ソート済み)
pub fn list() -> Vec<String> {
    list_json_names(&palettes_dir())
}

pub fn save(name: &str, palette: &Palette) -> Result<PathBuf, String> {
    let dir = palettes_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(palette).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{} に書けません: {e}", path.display()))?;
    Ok(path)
}

pub fn load(name: &str) -> Result<Palette, String> {
    let path = palettes_dir().join(format!("{name}.json"));
    let json = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    serde_json::from_str(&json).map_err(|e| format!("{name}.json の形式が不正です: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JSON 往復でパレットが保たれること(serde レイアウトの回帰チェック)
    #[test]
    fn roundtrip() {
        let pal = Palette::default_palette();
        let json = serde_json::to_string(&pal).unwrap();
        let back: Palette = serde_json::from_str(&json).unwrap();
        assert_eq!(pal, back);
    }

    /// リポジトリ同梱のパレットが全部読めること(壊れた JSON のコミットを防ぐ)
    #[test]
    fn bundled_palettes_load() {
        for name in list() {
            load(&name).unwrap_or_else(|e| panic!("パレット {name}: {e}"));
        }
    }
}
