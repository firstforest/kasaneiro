//! WGSL ホットリロード(H1)のファイル監視部分。
//! assets/shaders/ を notify で監視し、.wgsl が変わったらフレームループ側で再ビルドさせる。

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

/// 実行時にロードするシェーダーディレクトリ。
/// 開発中は cargo プロジェクト直下の assets/shaders を指す(どこから起動しても効くように
/// CARGO_MANIFEST_DIR 基準)。存在しなければカレントディレクトリ相対にフォールバック。
pub fn shader_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/shaders");
    if manifest.is_dir() {
        manifest
    } else {
        PathBuf::from("assets/shaders")
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
            if let Ok(event) = event {
                if event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|ext| ext == "wgsl"))
                {
                    dirty = true;
                }
            }
        }
        dirty
    }
}
