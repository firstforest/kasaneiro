//! WGSL ホットリロード(H1)のファイル監視部分。
//! assets/shaders/ を notify で監視し、.wgsl が変わったらフレームループ側で再ビルドさせる。

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

/// 実行時にロードするシェーダーディレクトリ(解決規則は assets.rs 参照)
pub fn shader_dir() -> PathBuf {
    crate::assets::asset_dir("shaders")
}

/// AI レビュー用スクショの出力先ディレクトリ(固定パス保存とトリガー監視の両方が使う)
pub fn screenshots_dir() -> PathBuf {
    crate::assets::base_dir().join("screenshots")
}

/// AI が撮影を指示するトリガーファイル名(screenshots/ 直下)。
/// 実行中のアプリに外部(AI の Bash)から「今の画面を撮れ」と伝える経路。
/// このファイルを作成/変更すると撮影が走る。アプリ自身が書く `ui-latest.png` は
/// 名前が違うので監視から無視され、保存が再撮影を呼ぶループにはならない。
pub const SHOT_REQUEST_FILE: &str = "request-shot";

/// スクショ撮影トリガーの監視(H6、AI レビュー用)。ShaderWatcher と同じ notify 方式で
/// screenshots/ を監視し、request-shot の作成/変更でフレームループ側に撮影を促す。
pub struct ScreenshotWatcher {
    // 監視を生かし続けるため保持する(drop すると監視が止まる)
    _watcher: Option<RecommendedWatcher>,
    rx: Option<Receiver<notify::Result<notify::Event>>>,
}

impl ScreenshotWatcher {
    /// dir(= screenshots/)を監視する。無ければ作る。失敗してもアプリは起動させる
    /// (AI からの撮影トリガーが効かないだけ。ボタンからの撮影は別経路で常に効く)。
    pub fn new(dir: &Path) -> Self {
        // 監視対象ディレクトリが無いと watch が失敗するので先に作る
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::warn!("スクショ監視ディレクトリを作れません({}): {e}", dir.display());
        }
        let (tx, rx) = channel();
        let watcher = notify::recommended_watcher(tx).and_then(|mut w| {
            w.watch(dir, RecursiveMode::NonRecursive)?;
            Ok(w)
        });
        match watcher {
            Ok(w) => Self {
                _watcher: Some(w),
                rx: Some(rx),
            },
            Err(e) => {
                log::warn!("スクショ監視の初期化に失敗({}): {e}", dir.display());
                Self {
                    _watcher: None,
                    rx: None,
                }
            }
        }
    }

    /// 前回呼び出し以降に撮影トリガー(request-shot の作成/変更)があったか。
    /// 削除イベントは無視する(応答としてトリガーを消しても再発火しないよう、書き込み系のみ拾う)。
    pub fn take_request(&mut self) -> bool {
        let Some(rx) = &self.rx else {
            return false;
        };
        let mut requested = false;
        while let Ok(event) = rx.try_recv() {
            if let Ok(event) = event
                && matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
                )
                && event
                    .paths
                    .iter()
                    .any(|p| p.file_name().is_some_and(|n| n == SHOT_REQUEST_FILE))
            {
                requested = true;
            }
        }
        requested
    }
}

pub struct ShaderWatcher {
    // 監視を生かし続けるため保持する(drop すると監視が止まる)
    _watcher: Option<RecommendedWatcher>,
    rx: Option<Receiver<notify::Result<notify::Event>>>,
}

impl ShaderWatcher {
    /// 監視の初期化に失敗してもアプリは起動させる(ホットリロードが効かないだけ)。
    pub fn new(dir: &Path) -> Self {
        // embed-assets(リリース配布)ではディスク上の .wgsl を使わないので監視しない
        // (編集が反映されないのに再ビルドだけ走る、という誤解を防ぐ)
        if cfg!(feature = "embed-assets") {
            return Self {
                _watcher: None,
                rx: None,
            };
        }
        let (tx, rx) = channel();
        let watcher = notify::recommended_watcher(tx).and_then(|mut w| {
            w.watch(dir, RecursiveMode::Recursive)?;
            Ok(w)
        });
        match watcher {
            Ok(w) => Self {
                _watcher: Some(w),
                rx: Some(rx),
            },
            Err(e) => {
                log::warn!("シェーダー監視の初期化に失敗({}): {e}", dir.display());
                Self {
                    _watcher: None,
                    rx: None,
                }
            }
        }
    }

    /// 前回呼び出し以降に .wgsl への変更イベントがあったか(イベントは読み捨てる)
    pub fn take_dirty(&mut self) -> bool {
        let Some(rx) = &self.rx else {
            return false;
        };
        let mut dirty = false;
        while let Ok(event) = rx.try_recv() {
            if let Ok(event) = event
                && event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|ext| ext == "wgsl"))
            {
                dirty = true;
            }
        }
        dirty
    }
}
