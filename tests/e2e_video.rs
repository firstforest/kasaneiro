//! 動画書き出しの e2e: headless で記録を再生しながら ffmpeg に流し、MP4 ができること・
//! 取り消すと書きかけが残らないことを見る。ffmpeg が起動できない環境では何もせず通す。
//! 書き出した動画は target/tmp/e2e/*.mp4 に残るので、目で確かめられる。

mod common;

use common::*;
use kasaneiro::video::{self, Progress, VideoSettings};
use paint_core::sim::SimParams;
use paint_core::tool::WetTool;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;

fn ffmpeg_available() -> bool {
    let ok = Command::new(video::ffmpeg_path())
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("ffmpeg が見つからないので動画書き出しのテストを飛ばします");
    }
    ok
}

fn out_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("e2e")
        .join(format!("{name}.mp4"))
}

fn settings() -> VideoSettings {
    VideoSettings {
        size: 512,
        params: SimParams::default(),
        steps_per_frame: 1,
        fps: video::VIDEO_FPS,
        tail_frames: 10,
    }
}

#[test]
fn exports_replay_to_mp4() {
    if !ffmpeg_available() {
        return;
    }
    let rec = recording(vec![
        hstroke(WetTool::Paint, 0, 120.0, 390.0, 230.0, 0),
        hstroke(WetTool::Paint, 1, 120.0, 390.0, 258.0, 60),
    ]);
    let expected = video::replay_frames(&rec) + settings().tail_frames;
    let path = out_path("replay_video");
    let progress = Progress::default();
    let mut c = canvas();
    video::encode(&mut c, rec, &settings(), &path, &progress).unwrap();

    let done = progress.done.load(Ordering::Relaxed);
    assert!(
        (expected..=expected + 1).contains(&done),
        "フレーム数が合わない: {done}(予定 {expected})"
    );
    let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    assert!(len > 0, "{} が空", path.display());
    // 白紙のままの動画でないこと: 最後のフレームに絵が載っている
    let last = c.snapshot().unwrap().mean_rgb(200, 220, 310, 270);
    assert!(
        lightness(last) < 0.9,
        "ストロークが描かれていない: {last:?}"
    );
}

#[test]
fn cancel_removes_partial_file() {
    if !ffmpeg_available() {
        return;
    }
    let rec = recording(vec![hstroke(WetTool::Paint, 0, 120.0, 390.0, 230.0, 0)]);
    let path = out_path("replay_video_cancel");
    let progress = Progress::default();
    progress.cancel.store(true, Ordering::Relaxed);
    let err = video::encode(&mut canvas(), rec, &settings(), &path, &progress).unwrap_err();
    assert!(err.contains("取り消し"), "{err}");
    assert!(!path.exists(), "書きかけ {} が残っている", path.display());
}
