//! 色単体ライブラリの保存/読込(M5f)。
//!
//! 顔料1個([`pigment::Pigment`] = 名前・基本色・ρ/ω/γ)を JSON で `assets/pigments/` に
//! 保存・読込する。パレット丸ごとの [`crate::palette_store`](M5d)と同じ流儀(アセット
//! ディレクトリ解決 = CARGO_MANIFEST_DIR 基準、`list_json_names` で一覧)。
//! **assets/pigments/*.json は git にコミットする**(「昨日作った色」を失わないため)。
//!
//! **保存名 = 顔料名の1本化**: [`save`] は書き込む前に `pigment.name` を保存名で上書きする。
//! 名前欄を別に設けず「1エンティティ1キー」を保つ(同名は黙って上書き=既存ストアと同じ割り切り)。
//! 欠落フィールドのある壊れた JSON は素直に読込エラー(palette_store と同方針。同梱色が
//! 全部読めることは cargo test で担保)。

use crate::assets::{asset_dir, list_json_names};
use pigment::Pigment;
use std::path::PathBuf;

pub fn pigments_dir() -> PathBuf {
    asset_dir("pigments")
}

/// 保存済みの色名の一覧(ソート済み)
pub fn list() -> Vec<String> {
    list_json_names(&pigments_dir())
}

pub fn save(name: &str, pigment: &Pigment) -> Result<PathBuf, String> {
    let dir = pigments_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.json"));
    // 保存名 = 顔料名(名前の正典を1本にする)。呼び出し側の name 欄と p.name のズレを残さない
    let mut p = pigment.clone();
    p.name = name.to_owned();
    let json = serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{} に書けません: {e}", path.display()))?;
    Ok(path)
}

pub fn load(name: &str) -> Result<Pigment, String> {
    let path = pigments_dir().join(format!("{name}.json"));
    let json = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    serde_json::from_str(&json).map_err(|e| format!("{name}.json の形式が不正です: {e}"))
}

/// 一覧+中身(色見本チップとホバーの ρ/ω/γ 表示用)。読めないファイルはスキップする。
/// モーダルを開くとき・保存後・↻ でのみ呼ぶ(ファイル監視はしない=既存キャッシュ流儀)
pub fn load_all() -> Vec<(String, Pigment)> {
    list()
        .into_iter()
        .filter_map(|name| load(&name).ok().map(|p| (name, p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JSON 往復で顔料が保たれること(serde レイアウトの回帰チェック)。
    /// save は name を保存名で上書きするので、その正規化も含めて検査する
    #[test]
    fn roundtrip() {
        let p = pigment::Palette::default_palette().pigments[0].clone();
        let json = serde_json::to_string(&p).unwrap();
        let back: Pigment = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    /// リポジトリ同梱の色が全部読めること + 保存名=顔料名の1本化が守られていること
    /// (壊れた JSON や名前のズレたファイルのコミットを防ぐ)
    #[test]
    fn bundled_pigments_load() {
        let all = load_all();
        assert!(!all.is_empty(), "同梱の色が1つもありません(assets/pigments/)");
        assert_eq!(all.len(), list().len(), "読めない同梱 JSON があります");
        for (name, p) in &all {
            assert_eq!(&p.name, name, "保存名と顔料名がズレています: {name}.json");
        }
    }
}
