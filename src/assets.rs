//! アセット・データディレクトリの解決と、配布ビルドの既定 JSON 書き出し。
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
