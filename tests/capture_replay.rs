//! 解析用キャプチャ(`--capture` で起動して残した `target/capture/<日時>/`)を headless で再生する。
//!
//! `cargo test --test capture_replay -- --ignored --nocapture`(`mise run replay:capture`)で、
//! `target/capture/` のいちばん新しいキャプチャ(`KASANEIRO_CAPTURE=<ディレクトリ>` があればそれ)を、起動時のパラメータ・記録時のパレットで `strokes.json` を流し、終了時と同じ刻みまで進めて
//! `replay.png` をそのディレクトリへ書く。アプリが終了時に撮った `canvas.png` と並べて、再生で再現できているかを見る
//! (乾燥操作・スライダーの変更は記録に入らないので、log.txt を見て差を判断する)。
//! headless は表示先を sRGB にしているので、非 sRGB の画面で撮った canvas.png より明るく写る。形と濃淡の並びで比べる

mod common;

use common::*;
use kasaneiro::capture::Session;
use kasaneiro::replay::StoredRecording;
use paint_core::sim::SimParams;
use std::path::PathBuf;

#[test]
#[ignore]
fn replay_capture() {
    let dir = std::env::var("KASANEIRO_CAPTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| latest_capture());
    let read = |name: &str| {
        std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name} を読めません: {e}"))
    };
    let stored: StoredRecording = serde_json::from_slice(&read("strokes.json")).unwrap();
    let params: SimParams = serde_json::from_slice(&read("params_start.json")).unwrap();
    let session: Session = serde_json::from_slice(&read("session.json")).unwrap();

    let rs = render_state();
    let mut c = kasaneiro::headless::HeadlessCanvas::new(rs.device, rs.queue, session.canvas_size)
        .expect("シェーダーのビルドに失敗");
    c.set_palette(stored.palette.clone());
    c.params = params;
    let strokes = stored.recording.strokes.len();
    let played = c.play(stored.recording);
    c.idle(session.frames.saturating_sub(played));
    let path = dir.join("replay.png");
    c.snapshot().unwrap().save_png(&path).unwrap();
    eprintln!(
        "ストローク {strokes} 本・{} 刻みを再生 → {}",
        session.frames.max(played),
        path.display()
    );
}

/// target/capture/ の中でいちばん新しいキャプチャ(ディレクトリ名が日時なので名前順の最後)
fn latest_capture() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .unwrap()
        .join("capture");
    std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{} を読めません: {e}", root.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("session.json").is_file())
        .max()
        .unwrap_or_else(|| panic!("{} に書き出し済みのキャプチャがありません", root.display()))
}
