//! GPU の所要時間の計測(回帰テストではない)。`#[ignore]` なので明示して回す:
//!
//! ```sh
//! mise exec -- cargo test --release --test perf_bench -- --ignored --nocapture
//! ```
//!
//! headless のキャンバスでフレームを積んで GPU の完了を待ち、1フレームあたりの壁時計時間を出す。
//! 見る場面:
//! - 乾いた紙で待機(何も濡れていない = 計算するタイルがない)
//! - 一部が濡れている(キャンバスサイズによらず同じ面積を濡らす)
//! - アクティブタイルを切った全面計算(active_tiles = 0)
//! - 表示(display.wgsl をキャンバス解像度で描く)を乾燥レイヤー 0 枚 / 8 枚で

mod common;

use common::*;
use kasaneiro::headless::HeadlessCanvas;
use kasaneiro::replay::{RecordedStroke, Recording, TimedPoint};
use paint_core::tool::WetTool;
use std::time::Instant;

const SIZES: [u32; 3] = [512, 1024, 2048];
const WARMUP: u32 = 10;
const SIM_FRAMES: u32 = 120;
const DISPLAY_FRAMES: u32 = 120;

/// 矩形 [x0,x1)×[y0,y1) を半径 r の筆で横縞に塗りつぶす記録(1フレームに r ずつ進む)
fn fill_rect(x0: f32, x1: f32, y0: f32, y1: f32, r: f32, channel: u32) -> Recording {
    let mut strokes = Vec::new();
    let mut frame = 0;
    let mut y = y0 + r * 0.5;
    while y < y1 {
        let mut points = Vec::new();
        let mut x = x0;
        while x <= x1 {
            points.push(TimedPoint {
                frame,
                pos: [x, y],
                pressure: 1.0,
            });
            frame += 1;
            x += r;
        }
        strokes.push(RecordedStroke {
            channel,
            tool: WetTool::Paint.gpu_id(),
            load: None,
            points,
        });
        frame += 2;
        y += r;
    }
    Recording { strokes }
}

fn new_canvas(size: u32) -> HeadlessCanvas {
    let rs = render_state();
    HeadlessCanvas::new(rs.device, rs.queue, size).expect("シェーダーのビルドに失敗")
}

/// 入力なしで frames フレーム進めたときの 1 フレームあたり ms
fn time_idle(c: &mut HeadlessCanvas, frames: u32) -> f64 {
    c.idle(WARMUP);
    c.wait_idle();
    let t = Instant::now();
    c.idle(frames);
    c.wait_idle();
    t.elapsed().as_secs_f64() * 1000.0 / frames as f64
}

fn time_display(c: &HeadlessCanvas, frames: u32) -> f64 {
    for _ in 0..WARMUP {
        c.render_display().unwrap();
    }
    c.wait_idle();
    let t = Instant::now();
    for _ in 0..frames {
        c.render_display().unwrap();
    }
    c.wait_idle();
    t.elapsed().as_secs_f64() * 1000.0 / frames as f64
}

#[test]
#[ignore = "計測用。--ignored --nocapture で回す"]
fn perf_bench() {
    println!(
        "| サイズ | 乾いた紙 | 一部濡れ(320²) | 全面計算 | 全面濡れの蒸発後 | 表示 層0 | 表示 層8 |"
    );
    println!("|---|---|---|---|---|---|---|");
    for size in SIZES {
        let mut c = new_canvas(size);
        let dry = time_idle(&mut c, SIM_FRAMES);

        // サイズによらず同じ 320×320 を濡らす(コストが濡れ面積比例なら 3 サイズで揃う)
        c.params.brush_radius = 24.0;
        c.play(fill_rect(96.0, 416.0, 96.0, 416.0, 24.0, 1));
        let wet = time_idle(&mut c, SIM_FRAMES);

        c.params.active_tiles = 0;
        let full = time_idle(&mut c, SIM_FRAMES / 2);
        c.params.active_tiles = 1;

        let display0 = time_display(&c, DISPLAY_FRAMES);

        // 全面を濡らして蒸発し切らせた後(濡れマスクは残る)。乾いた紙と同じ速さに戻るかを見る。
        // 水が尽きてから浮遊顔料が沈み切る(1ステップ約5%ずつ)まで数百ステップ待つ
        let r = size as f32 / 8.0;
        c.params.brush_radius = r;
        c.play(fill_rect(0.0, size as f32, 0.0, size as f32, r, 2));
        let evap = c.params.evap_rate;
        c.params.evap_rate = 0.05;
        c.idle(600);
        c.params.evap_rate = evap;
        let evaporated = time_idle(&mut c, SIM_FRAMES);

        // 全面を塗っては乾かすのを 8 回(表示のコストは顔料が載ったテクセルで決まる)
        for layer in 0..8 {
            c.play(fill_rect(0.0, size as f32, 0.0, size as f32, r, layer % 4));
            c.bake_dry().unwrap();
        }
        c.params.brush_radius = 16.0;
        // 乾燥レイヤーの合成キャッシュを焼かせる(表示の計測に焼き直しを混ぜない)
        c.idle(1);
        let display8 = time_display(&c, DISPLAY_FRAMES);

        println!(
            "| {size}² | {dry:.2} ms | {wet:.2} ms | {full:.2} ms | {evaporated:.2} ms | {display0:.2} ms | {display8:.2} ms |"
        );
    }
}
