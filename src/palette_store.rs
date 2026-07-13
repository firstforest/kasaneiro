//! パレット・ライブラリの保存/読込(M5d)。
//!
//! ランタイム編集した顔料パレット([`pigment::Palette`])を JSON で `assets/palettes/` に
//! 保存・読込する。プリセット(H3、[`crate::preset`])・ストローク(H5、[`crate::replay`])と
//! 同じ流儀(アセットディレクトリ解決 = assets.rs、`list_json_names` で一覧)。
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

/// 保存済みパレットを削除する(ファイルごと消す)。パレット一覧の「…」メニューから使う。
/// assets/palettes/ は git 管理なので、誤削除してもコミット済みなら復元できる
pub fn delete(name: &str) -> Result<(), String> {
    let path = palettes_dir().join(format!("{name}.json"));
    std::fs::remove_file(&path).map_err(|e| format!("{} を削除できません: {e}", path.display()))
}

pub fn load(name: &str) -> Result<Palette, String> {
    let path = palettes_dir().join(format!("{name}.json"));
    let json = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    serde_json::from_str(&json).map_err(|e| format!("{name}.json の形式が不正です: {e}"))
}

/// 一覧+中身(パレットモーダルの4色見本チップ用。M5g)。読めないファイルはスキップする。
/// モーダルを開くとき・保存後・↻ でのみ呼ぶ(ファイル監視はしない=既存キャッシュ流儀)
pub fn load_all() -> Vec<(String, Palette)> {
    list()
        .into_iter()
        .filter_map(|name| load(&name).ok().map(|p| (name, p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// assets/palettes/ を実際に触るテストの直列化(一時パレットの save/delete と同梱一覧の
    /// 検査が並列に走ると、一覧と読込の間で件数が食い違う)
    static DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// JSON 往復でパレットが保たれること(serde レイアウトの回帰チェック)
    #[test]
    fn roundtrip() {
        let pal = Palette::default_palette();
        let json = serde_json::to_string(&pal).unwrap();
        let back: Palette = serde_json::from_str(&json).unwrap();
        assert_eq!(pal, back);
    }

    /// save → delete の往復(一覧に現れて、消えること)。テスト名は同梱パレットと衝突しない一時名
    #[test]
    fn save_then_delete() {
        let _guard = DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let name = "テスト用一時パレット(自動テスト)";
        let pal = Palette::default_palette();
        save(name, &pal).unwrap();
        assert!(list().contains(&name.to_owned()));
        delete(name).unwrap();
        assert!(!list().contains(&name.to_owned()));
        assert!(delete(name).is_err(), "二重削除はエラーを返すはず");
    }

    /// リポジトリ同梱のパレットが全部読めること(壊れた JSON のコミットを防ぐ)
    #[test]
    fn bundled_palettes_load() {
        let _guard = DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for name in list() {
            load(&name).unwrap_or_else(|e| panic!("パレット {name}: {e}"));
        }
    }
}
