//! 色単体ライブラリの保存/読込(M5f)。
//!
//! 顔料1個([`pigment::Pigment`] = 名前・基本色・ρ/ω/γ)を JSON で `assets/pigments/` に
//! 保存・読込する。パレット丸ごとの [`crate::palette_store`](M5d)と同じ流儀(アセット
//! ディレクトリ解決 = assets.rs、`list_json_names` で一覧)。
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

/// 保存済みの色を削除する(ファイルごと消す)。色ライブラリの右クリックメニューから使う。
/// assets/pigments/ は git 管理なので、誤削除してもコミット済みなら復元できる
pub fn delete(name: &str) -> Result<(), String> {
    let path = pigments_dir().join(format!("{name}.json"));
    std::fs::remove_file(&path).map_err(|e| format!("{} を削除できません: {e}", path.display()))
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

    /// assets/pigments/ を実際に触るテストの直列化(一時色の save/delete と同梱一覧の
    /// 検査が並列に走ると、一覧と読込の間で件数が食い違う)
    static DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// JSON 往復で顔料が保たれること(serde レイアウトの回帰チェック)。
    /// save は name を保存名で上書きするので、その正規化も含めて検査する
    #[test]
    fn roundtrip() {
        let p = pigment::Palette::default_palette().pigments[0].clone();
        let json = serde_json::to_string(&p).unwrap();
        let back: Pigment = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    /// save → delete の往復(一覧に現れて、消えること)。テスト名は同梱色と衝突しない一時名
    #[test]
    fn save_then_delete() {
        let _guard = DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let name = "テスト用一時色(自動テスト)";
        let p = pigment::Palette::default_palette().pigments[0].clone();
        save(name, &p).unwrap();
        assert!(list().contains(&name.to_owned()));
        delete(name).unwrap();
        assert!(!list().contains(&name.to_owned()));
        assert!(delete(name).is_err(), "二重削除はエラーを返すはず");
    }

    /// リポジトリ同梱の色が全部読めること + 保存名=顔料名の1本化が守られていること
    /// (壊れた JSON や名前のズレたファイルのコミットを防ぐ)
    #[test]
    fn bundled_pigments_load() {
        let _guard = DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let all = load_all();
        assert!(!all.is_empty(), "同梱の色が1つもありません(assets/pigments/)");
        assert_eq!(all.len(), list().len(), "読めない同梱 JSON があります");
        for (name, p) in &all {
            assert_eq!(&p.name, name, "保存名と顔料名がズレています: {name}.json");
        }
    }
}
