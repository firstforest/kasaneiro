//! ストローク記録の動画書き出し。
//!
//! 画面のキャンバスとは別に [`HeadlessCanvas`] を作り、アプリの再生(H5)と同じ経路で記録を流す。
//! 1シミュレーションフレーム = 動画の1フレームとして見た目([`HeadlessCanvas::snapshot`])を読み戻し、
//! 外部の ffmpeg の stdin へ生の RGBA で流して MP4(H.264)にする。
//! 書き出しは別スレッドで走らせ、UI は [`VideoJob`] で進捗を見て、取り消せる。

use crate::headless::HeadlessCanvas;
use crate::replay::{Player, Recording};
use eframe::egui_wgpu::wgpu;
use paint_core::load::BrushFrame;
use paint_core::sim::SimParams;
use pigment::Palette;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;

/// 動画のフレームレート。記録のフレーム番号はアプリの描画フレームなので、一般的なモニタの 60Hz に合わせる
pub const VIDEO_FPS: u32 = 60;

/// 再生し終えてから撮り続けるフレーム数。最後のストロークのにじみ・広がりが落ち着くまでを映す
pub const TAIL_FRAMES: u32 = 2 * VIDEO_FPS;

/// ffmpeg の実行ファイルを上書き指定する環境変数
pub const FFMPEG_ENV: &str = "KASANEIRO_FFMPEG";

/// 書き出しの条件。キャンバスサイズ・パラメータは書き出し開始時のアプリの値を写す
#[derive(Clone)]
pub struct VideoSettings {
    pub size: u32,
    pub params: SimParams,
    pub steps_per_frame: u32,
    pub fps: u32,
    pub tail_frames: u32,
}

/// 動画の出力先ディレクトリ
pub fn videos_dir() -> PathBuf {
    crate::assets::base_dir().join("videos")
}

/// 使う ffmpeg: 環境変数 [`FFMPEG_ENV`] → exe(開発時はリポジトリ)隣の ffmpeg → PATH の ffmpeg
pub fn ffmpeg_path() -> PathBuf {
    if let Some(p) = std::env::var_os(FFMPEG_ENV) {
        return PathBuf::from(p);
    }
    let local = crate::assets::base_dir().join(if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    });
    if local.is_file() {
        return local;
    }
    PathBuf::from("ffmpeg")
}

/// 記録の再生にかかるフレーム数(最後の点のフレーム + 1)
pub fn replay_frames(recording: &Recording) -> u32 {
    recording
        .strokes
        .iter()
        .flat_map(|s| s.points.iter())
        .map(|p| p.frame + 1)
        .max()
        .unwrap_or(0)
}

/// 書き出しの進捗を共有する
#[derive(Default)]
pub struct Progress {
    /// 書き終えたフレーム数
    pub done: AtomicU32,
    /// 立っていたら次のフレームの前で打ち切る
    pub cancel: AtomicBool,
}

/// `canvas` で記録を再生しながら `path` へ動画を書き出す。`canvas` は白紙・記録時パレット設定済みのもの。
/// 取り消されたら書きかけのファイルを消して Err を返す
pub fn encode(
    canvas: &mut HeadlessCanvas,
    recording: Recording,
    settings: &VideoSettings,
    path: &Path,
    progress: &Progress,
) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    }
    let mut ffmpeg = Ffmpeg::spawn(canvas.size(), settings.fps, path)?;
    let mut player = Some(Player::new(recording));
    let mut tail = 0;
    let result = (|| {
        loop {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err("動画の書き出しを取り消しました".to_owned());
            }
            let mut splats = Vec::new();
            let mut brush = BrushFrame::default();
            match &mut player {
                Some(p) => {
                    if !p.advance(&mut canvas.params, &mut splats, &mut brush) {
                        player = None;
                    }
                }
                None if tail < settings.tail_frames => tail += 1,
                None => return Ok(()),
            }
            canvas.frame(splats, brush);
            let image = canvas.snapshot()?;
            ffmpeg.write_frame(&image.rgba)?;
            progress.done.fetch_add(1, Ordering::Relaxed);
        }
    })();
    match result {
        Ok(()) => ffmpeg.finish(),
        Err(e) => {
            ffmpeg.abort();
            let _ = std::fs::remove_file(path);
            Err(e)
        }
    }
}

/// stdin に生の RGBA を受けて H.264 の MP4 を書く ffmpeg の子プロセス
struct Ffmpeg {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    /// stderr はパイプが詰まらないよう別スレッドで読み切り、失敗時のメッセージに使う
    stderr: Option<JoinHandle<String>>,
}

impl Ffmpeg {
    fn spawn(size: u32, fps: u32, path: &Path) -> Result<Self, String> {
        let exe = ffmpeg_path();
        let mut cmd = Command::new(&exe);
        cmd.args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{size}x{size}")])
            .args(["-framerate", &fps.to_string()])
            .args(["-i", "-"])
            .args(["-c:v", "libx264", "-preset", "medium", "-crf", "18"])
            .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            // GUI アプリから起動したときにコンソールウィンドウを開かせない(CREATE_NO_WINDOW)
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "ffmpeg を起動できません({}: {e})。ffmpeg を PATH に通すか、exe の隣に置くか、環境変数 {FFMPEG_ENV} で場所を指定してください",
                exe.display()
            )
        })?;
        let stdin = child.stdin.take();
        let stderr = child.stderr.take().map(|mut err| {
            std::thread::spawn(move || {
                let mut s = String::new();
                let _ = err.read_to_string(&mut s);
                s
            })
        });
        Ok(Self {
            child,
            stdin,
            stderr,
        })
    }

    fn write_frame(&mut self, rgba: &[u8]) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("ffmpeg の入力が閉じています")?;
        if let Err(e) = stdin.write_all(rgba) {
            // 書き込みの失敗は ffmpeg 側が落ちたとき。理由は stderr にある
            self.stdin = None;
            let _ = self.child.wait();
            return Err(format!(
                "ffmpeg への書き込みに失敗: {e}\n{}",
                self.take_stderr()
            ));
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        self.stdin = None; // EOF で ffmpeg に書き終わりを伝える
        let status = self
            .child
            .wait()
            .map_err(|e| format!("ffmpeg の終了待ちに失敗: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "ffmpeg が失敗しました({status})\n{}",
                self.take_stderr()
            ))
        }
    }

    fn abort(mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn take_stderr(&mut self) -> String {
        self.stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default()
            .trim()
            .to_owned()
    }
}

/// 別スレッドで走っている書き出し
pub struct VideoJob {
    pub name: String,
    pub path: PathBuf,
    /// 予定の総フレーム数(再生 + 余韻)
    pub total: u32,
    progress: Arc<Progress>,
    handle: Option<JoinHandle<Result<(), String>>>,
}

impl VideoJob {
    /// 書き出しを始める。`device` / `queue` は画面と共有してよい(キャンバスは別に作る)
    pub fn spawn(
        device: wgpu::Device,
        queue: wgpu::Queue,
        name: String,
        recording: Recording,
        palette: Palette,
        settings: VideoSettings,
        path: PathBuf,
    ) -> Self {
        let total = replay_frames(&recording) + settings.tail_frames;
        let progress = Arc::new(Progress::default());
        let shared = Arc::clone(&progress);
        let out = path.clone();
        let handle = std::thread::spawn(move || {
            let mut canvas = HeadlessCanvas::new(device, queue, settings.size)?;
            canvas.params = settings.params;
            canvas.steps_per_frame = settings.steps_per_frame;
            canvas.set_palette(palette);
            encode(&mut canvas, recording, &settings, &out, &shared)
        });
        Self {
            name,
            path,
            total,
            progress,
            handle: Some(handle),
        }
    }

    /// 書き終えたフレーム数
    pub fn done(&self) -> u32 {
        self.progress.done.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.progress.cancel.store(true, Ordering::Relaxed);
    }

    /// 終わっていれば結果を返す(1回だけ)。走っている間は None
    pub fn poll(&mut self) -> Option<Result<PathBuf, String>> {
        if !self.handle.as_ref()?.is_finished() {
            return None;
        }
        let result = match self.handle.take()?.join() {
            Ok(r) => r.map(|()| self.path.clone()),
            Err(_) => Err("動画の書き出しスレッドが異常終了しました".to_owned()),
        };
        Some(result)
    }
}
