//! アセット・データディレクトリの解決、配布ビルドの既定 JSON 書き出し、名前付きファイルの保存(保存名の正規化・アトミック書き込み・JSON ストア共通の save/load/delete/list)。
//! shaders(H1)・presets(H3)・strokes(H5)・pigments/palettes(M5)・works(M7)・
//! snapshots/screenshots(H6)が基準ディレクトリを共用する。

use std::path::{Path, PathBuf};

/// アプリのデータ基準ディレクトリ。assets/ works/ snapshots/ screenshots/ はすべてこの直下。
/// 通常ビルド: CARGO_MANIFEST_DIR = リポジトリ直下(どこから起動しても効く)。
#[cfg(not(feature = "embed-assets"))]
pub fn base_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// 配布ビルド(embed-assets): exe のあるディレクトリ(ポータブルアプリ流儀)。
/// CWD 基準にしない — ショートカットやターミナルからの起動で CWD は exe の場所と一致しない
#[cfg(feature = "embed-assets")]
pub fn base_dir() -> &'static Path {
    use std::sync::OnceLock;
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."))
    })
}

/// assets/ 配下のサブディレクトリを返す(存在しなくてもよい。presets 等は保存時に作られる)
pub fn asset_dir(sub: &str) -> PathBuf {
    base_dir().join("assets").join(sub)
}

/// 配布ビルド(embed-assets)に既定 JSON アセットを埋め込む。
/// 起動時に seed_default_assets が exe 隣へ書き出して初期状態を作る
#[cfg(feature = "embed-assets")]
static EMBEDDED_JSON_ASSETS: [(&str, include_dir::Dir<'static>); 4] = [
    ("pigments", include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/pigments")),
    ("palettes", include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/palettes")),
    ("presets", include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/presets")),
    ("strokes", include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/strokes")),
];

/// 初回起動時、既定 JSON アセット(顔料・パレット・プリセット・ストローク)を書き出す。
/// **ディレクトリ単位**で「無ければ書く」— 既にあれば一切触らない(ユーザーの編集・削除を
/// 起動のたびに上書き・復活させないため)。失敗しても起動は続ける(該当機能が空になるだけ)。
#[cfg(feature = "embed-assets")]
pub fn seed_default_assets() {
    for (sub, dir) in &EMBEDDED_JSON_ASSETS {
        let target = asset_dir(sub);
        if target.is_dir() {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&target) {
            log::warn!("{} を作れません: {e}", target.display());
            continue;
        }
        for file in dir.files() {
            let path = target.join(file.path());
            if let Err(e) = std::fs::write(&path, file.contents()) {
                log::warn!("既定アセットを書き出せません({}): {e}", path.display());
            }
        }
    }
}

/// 通常ビルドでは何もしない(assets/ はリポジトリにある)
#[cfg(not(feature = "embed-assets"))]
pub fn seed_default_assets() {}

/// 保存名をファイル名として安全な形に正規化する。全ストア(作品・プリセット・パレット・色・
/// ストローク)の保存はここを通す。
///
/// - パス区切り(`/` `\`)と Windows の禁止文字(`: * ? " < > |`)・制御文字を `_` に置換
///   (`..\x` のような保存先の外を指す名前も、ディレクトリ内の1ファイル名になる)
/// - 前後の空白と末尾のドットを除去(Windows は末尾の `.` / 空白を落として扱う)
/// - Windows の予約デバイス名(CON / NUL / COM1 など。拡張子付きでも予約)は先頭に `_` を付ける
///
/// 結果が空なら Err(保存しない)。
pub fn sanitize_name(name: &str) -> Result<String, String> {
    let replaced: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = replaced.trim().trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return Err(format!("「{name}」はファイル名に使えません"));
    }
    let stem = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    let numbered_device = (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str()) || numbered_device {
        return Ok(format!("_{trimmed}"));
    }
    Ok(trimmed.to_owned())
}

/// 一覧から来た名前(読込・削除)の検査。保存先の外を指すもの(パス区切り・`.`・`..`)だけを
/// 拒否する(保存時の正規化より緩い = 手で置いた既存ファイルも読める)
fn check_existing_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(format!("名前「{name}」は使えません"));
    }
    Ok(())
}

/// バイト列をアトミックに書く: 同じディレクトリの `{path}.tmp` に書いてから rename で置き換える。
/// 書き込み途中で落ちても既存ファイルは壊れない
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| format!("{} に書けません: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{} へ置き換えられません: {e}", path.display())
    })
}

/// `dir/{name}.{ext}` へアトミックに保存する。名前は [`sanitize_name`] を通す
/// (UI は名前欄へ反映するため、同じ関数で正規化した名前を渡す)
pub fn save_named(dir: &Path, name: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let name = sanitize_name(name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.{ext}"));
    write_atomic(&path, bytes)?;
    Ok(path)
}

/// `dir/{name}.{ext}` が既にあるか(上書き確認用)。名前は保存時と同じく正規化して判定する
pub fn named_exists(dir: &Path, name: &str, ext: &str) -> bool {
    sanitize_name(name).is_ok_and(|n| dir.join(format!("{n}.{ext}")).is_file())
}

/// `dir/{name}.{ext}` を読む(名前は一覧から来たもの)
pub fn read_named(dir: &Path, name: &str, ext: &str) -> Result<Vec<u8>, String> {
    check_existing_name(name)?;
    let path = dir.join(format!("{name}.{ext}"));
    std::fs::read(&path).map_err(|e| format!("{} を読めません: {e}", path.display()))
}

/// dir 直下の `.{ext}` のファイル名(拡張子なし)をソートして返す。ディレクトリが無ければ空
pub fn list_names(dir: &Path, ext: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            if path.extension().is_some_and(|x| x == ext) {
                Some(path.file_stem()?.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

/// JSON ストア(プリセット・パレット・色・ストローク)の拡張子
const JSON_EXT: &str = "json";

/// 値を `dir/{name}.json` へアトミックに保存する。`pretty` は人が読む・git で差分を見る
/// 設定系で true、サイズの大きいストローク記録で false
pub fn json_save<T: serde::Serialize>(
    dir: &Path,
    name: &str,
    value: &T,
    pretty: bool,
) -> Result<PathBuf, String> {
    let json = if pretty {
        serde_json::to_vec_pretty(value)
    } else {
        serde_json::to_vec(value)
    }
    .map_err(|e| e.to_string())?;
    save_named(dir, name, JSON_EXT, &json)
}

/// `dir/{name}.json` を読んで値に戻す
pub fn json_load<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Result<T, String> {
    let bytes = read_named(dir, name, JSON_EXT)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{name}.json の形式が不正です: {e}"))
}

/// `dir/{name}.json` を削除する
pub fn json_delete(dir: &Path, name: &str) -> Result<(), String> {
    check_existing_name(name)?;
    let path = dir.join(format!("{name}.{JSON_EXT}"));
    std::fs::remove_file(&path).map_err(|e| format!("{} を削除できません: {e}", path.display()))
}

/// dir 直下の .json のファイル名(拡張子なし・ソート済み)
pub fn json_list(dir: &Path) -> Vec<String> {
    list_names(dir, JSON_EXT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_forbidden_chars() {
        assert_eq!(
            sanitize_name("a/b\\c:d*e?f\"g<h>i|j").unwrap(),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_name("タブ\t改行\nX").unwrap(), "タブ_改行_X");
        assert_eq!(sanitize_name("  夕焼け  ").unwrap(), "夕焼け");
    }

    /// 保存先の外を指す名前は区切りが潰れてディレクトリ内の1ファイル名になる
    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(sanitize_name("..\\..\\x").unwrap(), ".._.._x");
        assert_eq!(sanitize_name("../x").unwrap(), ".._x");
    }

    #[test]
    fn sanitize_rejects_empty_and_dots() {
        for bad in ["", "   ", ".", "..", "...", " . "] {
            assert!(sanitize_name(bad).is_err(), "{bad:?} は拒否されるはず");
        }
        assert_eq!(sanitize_name("名前.").unwrap(), "名前");
    }

    #[test]
    fn sanitize_escapes_reserved_device_names() {
        assert_eq!(sanitize_name("CON").unwrap(), "_CON");
        assert_eq!(sanitize_name("nul.txt").unwrap(), "_nul.txt");
        assert_eq!(sanitize_name("com1").unwrap(), "_com1");
        assert_eq!(sanitize_name("CONSOLE").unwrap(), "CONSOLE");
        assert_eq!(sanitize_name("COMA").unwrap(), "COMA");
    }

    #[test]
    fn existing_name_rejects_paths() {
        assert!(check_existing_name("../x").is_err());
        assert!(check_existing_name("a\\b").is_err());
        assert!(check_existing_name("..").is_err());
        assert!(check_existing_name("夕焼け").is_ok());
    }

    /// 保存 → 一覧 → 読込 → 上書き → 削除の往復と、アトミック書き込みの .tmp が残らないこと
    #[test]
    fn json_store_roundtrip() {
        let dir = std::env::temp_dir()
            .join(format!("kasaneiro-assets-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = json_save(&dir, "a/b", &vec![1u32, 2, 3], true).unwrap();
        assert_eq!(path, dir.join("a_b.json"));
        assert_eq!(json_list(&dir), vec!["a_b".to_owned()]);
        assert!(named_exists(&dir, "a/b", "json"));
        let back: Vec<u32> = json_load(&dir, "a_b").unwrap();
        assert_eq!(back, vec![1, 2, 3]);
        json_save(&dir, "a_b", &vec![4u32], false).unwrap();
        let back: Vec<u32> = json_load(&dir, "a_b").unwrap();
        assert_eq!(back, vec![4]);
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "tmp"))
            .count();
        assert_eq!(leftovers, 0, ".tmp が残っています");
        json_delete(&dir, "a_b").unwrap();
        assert!(json_list(&dir).is_empty());
        assert!(json_save(&dir, " .. ", &0u32, true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
