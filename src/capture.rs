//! 解析用キャプチャ(`--capture` 付きで起動したときだけ動く)。
//!
//! 起動直後から終了までを `target/capture/<日時>/` に残す。不具合の報告を「描いて閉じるだけ」にして、
//! 後から headless で再生・数値で調べられるようにするのが目的:
//!
//! - `log.txt`: アプリのログ(標準エラーにも同じものを出す)。ストロークの開始・終了、ツール・
//!   パラメータの変化、乾燥操作、ステータス表示の変化を、記録の刻み番号(`frame`)付きで書く
//! - `strokes.json`: 起動直後からの全ストローク([`crate::replay::StoredRecording`]。終了時のパレット付き)
//! - `params_start.json` / `params_end.json`: 起動時・終了時の `SimParams`
//! - `session.json`: 終了までの刻み数とキャンバスの大きさ([`Session`]。再生を終了時と同じ時点まで進めるため)
//! - 終了時: `ui.png`(ウィンドウ全体)・`canvas.png`(キャンバス全体)・`work.kasane`(作品ファイル =
//!   湿レイヤーの水・顔料を含む全状態)
//!
//! 刻み番号は記録([`Recorder`])の刻みと同じなので、ログの行と `strokes.json` の点を突き合わせられる。

use crate::replay::{Recorder, StoredRecording};
use paint_core::sim::SimParams;
use pigment::Palette;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// コマンドライン引数に `--capture` があれば、保存先のディレクトリを作って返す
pub fn dir_from_args() -> Option<PathBuf> {
    if !std::env::args().skip(1).any(|a| a == "--capture") {
        return None;
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dir = capture_root().join(stamp.to_string());
    match std::fs::create_dir_all(&dir) {
        Ok(()) => Some(dir),
        Err(e) => {
            eprintln!("キャプチャ先 {} を作れません: {e}", dir.display());
            None
        }
    }
}

/// 起動した checkout の `target/capture`(exe の祖先の `target`)。見つからなければ基準ディレクトリ直下
fn capture_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.ancestors()
                .find(|p| p.file_name().is_some_and(|n| n == "target"))
                .map(Path::to_path_buf)
        })
        .unwrap_or_else(|| crate::assets::base_dir().to_path_buf())
        .join("capture")
}

/// ログを初期化する。ふだんは error だけ。キャプチャ中は `log.txt` にも書き、自前のクレートを debug まで、
/// ほかを warn まで出す(wgpu の内部はローダーの警告が大量に出るので error まで)。RUST_LOG があればそれで上書きする
pub fn init_logging(dir: Option<&Path>) {
    let mut builder = env_logger::Builder::new();
    builder.filter_level(log::LevelFilter::Error);
    if dir.is_some() {
        builder
            .filter_level(log::LevelFilter::Warn)
            .filter_module("wgpu_hal", log::LevelFilter::Error)
            .filter_module("wgpu_core", log::LevelFilter::Error)
            .filter_module("kasaneiro", log::LevelFilter::Debug)
            .filter_module("paint_core", log::LevelFilter::Debug);
    }
    if let Ok(spec) = std::env::var("RUST_LOG") {
        builder.parse_filters(&spec);
    }
    if let Some(dir) = dir {
        match std::fs::File::create(dir.join("log.txt")) {
            Ok(file) => {
                builder.target(env_logger::Target::Pipe(Box::new(Tee(Arc::new(
                    Mutex::new(file),
                )))));
            }
            Err(e) => eprintln!("log.txt を作れません: {e}"),
        }
    }
    builder.init();
    if dir.is_some() {
        // パニックもログファイルへ残す(標準の表示はそのまま続ける)
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("パニック: {info}");
            default(info);
        }));
    }
}

/// 標準エラーとファイルの両方へ書く
struct Tee(Arc<Mutex<std::fs::File>>);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        let mut file = self.0.lock().unwrap_or_else(|e| e.into_inner());
        file.write_all(buf)?;
        file.flush()?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).flush()
    }
}

/// 記録全体の情報。再生は `strokes.json` を流した後、`frames` まで何も描かずに進めると終了時と同じ時点になる
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Session {
    /// 起動から終了までの刻み数(記録の刻みと同じ数え方)
    pub frames: u32,
    pub canvas_size: u32,
}

/// 終了時の書き出しの進み具合
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Closing {
    /// 描いている最中
    Running,
    /// 閉じる要求を止めて書き出し、UI スクショの到着を待っている(待った刻み数)
    WaitingShot(u32),
    /// 書き出し済み。次の閉じる要求はそのまま通す
    Done,
}

/// UI スクショを待つ上限(フレーム)。届かなくても閉じる
pub const SHOT_TIMEOUT_FRAMES: u32 = 60;
/// パラメータが変わってからこのフレーム数動かなければ、変化をまとめて1行に書く(スライダーのドラッグで毎フレーム書かない)
const PARAMS_SETTLE_FRAMES: u32 = 20;

/// キャプチャ中の状態
pub struct Capture {
    pub dir: PathBuf,
    pub recorder: Recorder,
    /// 記録の刻み番号(ログの行に付ける)
    pub frame: u32,
    pub closing: Closing,
    /// ログに書いた最後のパラメータ
    logged_params: SimParams,
    /// まだ書いていない変化: 最後に見た値と、その値のまま動かなかったフレーム数
    params_pending: Option<(SimParams, u32)>,
    /// 記録したストロークの本数(ログの通し番号)
    strokes: usize,
    last_tool: String,
    last_status: Option<String>,
    /// 終了時の書き出しを済ませたか(UI スクショを除く)
    flushed: bool,
}

impl Capture {
    pub fn new(dir: PathBuf, params: &SimParams) -> Self {
        write_json(&dir.join("params_start.json"), params);
        log::info!("キャプチャ開始: {}", dir.display());
        Self {
            dir,
            recorder: Recorder::new(),
            frame: 0,
            closing: Closing::Running,
            logged_params: *params,
            params_pending: None,
            strokes: 0,
            last_tool: String::new(),
            last_status: None,
            flushed: false,
        }
    }

    pub fn tick(&mut self) {
        self.recorder.tick();
        self.frame += 1;
    }

    /// 毎フレーム: ツール・パラメータ・ステータス表示の変化をログへ
    pub fn observe(&mut self, tool: &str, params: &SimParams, status: Option<&String>) {
        if tool != self.last_tool {
            log::info!("[frame {}] ツール: {tool}", self.frame);
            self.last_tool = tool.to_owned();
        }
        if status != self.last_status.as_ref() {
            if let Some(msg) = status {
                log::info!("[frame {}] 表示: {msg}", self.frame);
            }
            self.last_status = status.cloned();
        }
        if *params == self.logged_params {
            self.params_pending = None;
            return;
        }
        // 変わり続けている間(スライダーのドラッグ中)は待ち、止まってから前回書いた値との差をまとめて書く
        match &mut self.params_pending {
            Some((seen, still)) if seen == params => {
                *still += 1;
                if *still >= PARAMS_SETTLE_FRAMES {
                    log::info!(
                        "[frame {}] パラメータ: {}",
                        self.frame,
                        params_diff(&self.logged_params, params)
                    );
                    self.logged_params = *params;
                    self.params_pending = None;
                }
            }
            _ => self.params_pending = Some((*params, 0)),
        }
    }

    pub fn stroke_begin(
        &mut self,
        tool: &str,
        channel: u32,
        load: Option<&paint_core::load::BrushLoad>,
    ) {
        log::info!(
            "[frame {}] ストローク開始 #{}: {tool} 色スロット {channel} 筆 {load:?}",
            self.frame,
            self.strokes
        );
        self.strokes += 1;
    }

    pub fn stroke_end(&self, load: &paint_core::load::BrushLoad) {
        log::info!("[frame {}] ストローク終了: 筆 {load:?}", self.frame);
    }

    /// 終了時の書き出し(UI スクショ以外)。2回目以降は何もしない
    pub fn flush(
        &mut self,
        params: &SimParams,
        palette: &Palette,
        canvas_size: u32,
        canvas_png: Result<Vec<u8>, String>,
        work: Result<Vec<u8>, String>,
    ) {
        if self.flushed {
            return;
        }
        self.flushed = true;
        if *params != self.logged_params {
            log::info!(
                "[frame {}] パラメータ: {}",
                self.frame,
                params_diff(&self.logged_params, params)
            );
        }
        write_json(&self.dir.join("params_end.json"), params);
        let recording = std::mem::take(&mut self.recorder).finish();
        let strokes = recording.strokes.len();
        let stored = StoredRecording {
            recording,
            palette: palette.clone(),
        };
        // 記録は大きいので整形しない
        match serde_json::to_vec(&stored) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(self.dir.join("strokes.json"), bytes) {
                    log::warn!("strokes.json を書けません: {e}");
                }
            }
            Err(e) => log::warn!("strokes.json を作れません: {e}"),
        }
        write_json(
            &self.dir.join("session.json"),
            &Session {
                frames: self.frame,
                canvas_size,
            },
        );
        match canvas_png {
            Ok(data) => save_png(&self.dir.join("canvas.png"), canvas_size, canvas_size, &data),
            Err(e) => log::warn!("キャンバスの PNG を書き出せません: {e}"),
        }
        match work {
            Ok(bytes) => {
                let path = self.dir.join(format!("work.{}", crate::work::EXT));
                if let Err(e) = std::fs::write(&path, bytes) {
                    log::warn!("{} を書けません: {e}", path.display());
                }
            }
            Err(e) => log::warn!("作品ファイルを書き出せません: {e}"),
        }
        log::info!(
            "[frame {}] キャプチャ書き出し: ストローク {strokes} 本 → {}",
            self.frame,
            self.dir.display()
        );
    }

    pub fn save_ui_shot(&self, image: &eframe::egui::ColorImage) {
        let [w, h] = image.size;
        save_png(&self.dir.join("ui.png"), w as u32, h as u32, image.as_raw());
    }
}

fn save_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    if let Err(e) = image::save_buffer(path, rgba, w, h, image::ExtendedColorType::Rgba8) {
        log::warn!("{} を書けません: {e}", path.display());
    }
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) {
    let result = serde_json::to_vec_pretty(value)
        .map_err(|e| e.to_string())
        .and_then(|bytes| std::fs::write(path, bytes).map_err(|e| e.to_string()));
    if let Err(e) = result {
        log::warn!("{} を書けません: {e}", path.display());
    }
}

/// 2つの SimParams の、値が違うフィールドだけを `名前: 前 → 後` で並べる
fn params_diff(before: &SimParams, after: &SimParams) -> String {
    let (Ok(serde_json::Value::Object(a)), Ok(serde_json::Value::Object(b))) =
        (serde_json::to_value(before), serde_json::to_value(after))
    else {
        return format!("{after:?}");
    };
    b.iter()
        .filter(|(k, v)| a.get(*k) != Some(*v))
        .map(|(k, v)| {
            format!(
                "{k}: {} → {v}",
                a.get(k).map_or("-".to_owned(), |x| x.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_diff_lists_only_changed_fields() {
        let a = SimParams::default();
        let mut b = a;
        b.brush_radius = a.brush_radius + 1.0;
        let diff = params_diff(&a, &b);
        assert!(diff.starts_with("brush_radius: "), "{diff}");
        assert!(!diff.contains(','), "{diff}");
    }
}
