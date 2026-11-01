//! 水彩シミュの e2e: ストロークを headless で再生し、3つのコア要件(wet-on-wet / グレージング /
//! 削り)を見た目の指標で判定する。
//!
//! シェーダーの挙動を細かく固定するテストではなく、「目で見て判定する体験」(plan.md 冒頭の設計方針)が
//! 壊れていないかを色相・明度の帯で見る回帰検知。パラメータ調整で色味が多少動いても通る幅にしてある。
//! 結果画像は target/tmp/e2e/*.png に書き出すので、落ちたらまず画像を見る。
//! 既定パレット: 0=ハンザイエロー / 1=フタロブルー / 2=キナクリドンマゼンタ / 3=バーントシェンナ

mod common;

use common::*;
use kasaneiro::headless::HeadlessCanvas;
use paint_core::tool::WetTool;

const YELLOW: u32 = 0;
const BLUE: u32 = 1;
const SIENNA: u32 = 3;

/// 緑とみなす色相の帯(黄 ≈ 60°、青 ≈ 200° の間)
const GREEN_HUE: std::ops::Range<f32> = 90.0..170.0;

fn paper(c: &HeadlessCanvas) -> [f32; 3] {
    c.snapshot().unwrap().mean_rgb(0, 0, 32, 32)
}

/// wet-on-wet: 濡れた黄と青を隣接させると、境界が緑のグラデーションで馴染む
/// (assets/strokes/two-color-adjacent.json: 黄 y=230 / 青 y=258、x=120..390)
#[test]
fn wet_on_wet_blends_into_green_gradient() {
    let mut c = canvas();
    let stored = kasaneiro::replay::load("two-color-adjacent").unwrap();
    c.set_palette(stored.palette);
    c.play(stored.recording);
    c.idle(120);
    let img = c.snapshot().unwrap();
    let png = save(&img, "wet_on_wet");

    // x=200..310(ストローク中央部)を 4 行ずつ平均した色相の縦断面
    let rows: Vec<(u32, [f32; 3])> = (216..272)
        .step_by(4)
        .map(|y| (y, img.mean_rgb(200, y, 310, y + 4)))
        .collect();
    let profile: Vec<String> = rows
        .iter()
        .map(|(y, m)| {
            let (h, s, v) = hsv(*m);
            format!("y={y} h={h:.0} s={s:.2} v={v:.2}")
        })
        .collect();
    let ctx = format!("{}\n{}", png.display(), profile.join("\n"));

    let (top_h, _, _) = hsv(rows.first().unwrap().1);
    let (bottom_h, _, _) = hsv(rows.last().unwrap().1);
    assert!((40.0..80.0).contains(&top_h), "上端が黄でない\n{ctx}");
    assert!((170.0..240.0).contains(&bottom_h), "下端が青でない\n{ctx}");
    assert!(
        rows.iter().any(|(_, m)| {
            let (h, s, _) = hsv(*m);
            GREEN_HUE.contains(&h) && s > 0.08
        }),
        "境界に緑が出ていない(混ざっていない)\n{ctx}"
    );
    // 境目がくっきり分かれず、色相が少しずつ移る(隣の行との差が小さい)
    let max_jump = rows
        .windows(2)
        .map(|w| (hsv(w[1].1).0 - hsv(w[0].1).0).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_jump < 45.0,
        "色相が段差で切り替わっている(最大 {max_jump:.0}°)\n{ctx}"
    );
    // 境界が紙の白で途切れていない
    let gap = rows
        .iter()
        .map(|(_, m)| lightness(*m))
        .fold(0.0f32, f32::max);
    assert!(
        gap < lightness(paper(&c)) - 0.05,
        "境界に白い隙間がある\n{ctx}"
    );
}

/// グレージング: 乾かした黄の上に青を重ねると、交差部だけ緑に濃くなり、下の層は変わらない
#[test]
fn glazing_layers_over_dried_paint() {
    let mut c = canvas();
    c.play(recording(vec![hstroke(
        WetTool::Paint,
        YELLOW,
        100.0,
        400.0,
        200.0,
        0,
    )]));
    c.idle(120);
    c.bake_dry().unwrap();
    let under = c.snapshot().unwrap();

    c.play(recording(vec![stroke(
        WetTool::Paint,
        BLUE,
        [250.0, 100.0],
        [250.0, 300.0],
        0,
    )]));
    c.idle(120);
    c.bake_dry().unwrap();
    let img = c.snapshot().unwrap();
    let png = save(&img, "glazing");

    assert_eq!(c.layer_count(), 2);
    let yellow_before = under.mean_rgb(120, 195, 180, 205);
    let yellow_after = img.mean_rgb(120, 195, 180, 205);
    let cross = img.mean_rgb(245, 195, 255, 205);
    let blue = img.mean_rgb(245, 120, 255, 140);
    let ctx = format!(
        "{}\n黄(前)={yellow_before:?} 黄(後)={yellow_after:?} 交差={cross:?} {:?} 青={blue:?}",
        png.display(),
        hsv(cross)
    );

    assert!(
        dist(yellow_before, yellow_after) < 0.01,
        "上塗りで乾いた下の層が動いた\n{ctx}"
    );
    let (h, s, _) = hsv(cross);
    assert!(
        GREEN_HUE.contains(&h) && s > 0.1,
        "交差部が緑に重なっていない\n{ctx}"
    );
    assert!(
        lightness(cross) < lightness(yellow_after).min(lightness(blue)),
        "交差部が黄・青どちらより濃くなっていない\n{ctx}"
    );
}

/// 完全消去: 濡れている色を消すと紙の白まで戻り、消していない部分は残る
#[test]
fn erase_restores_paper_white() {
    let mut c = canvas();
    let paper = paper(&c);
    c.play(recording(vec![hstroke(
        WetTool::Paint,
        YELLOW,
        100.0,
        400.0,
        256.0,
        0,
    )]));
    c.idle(30);
    c.play(recording(vec![hstroke(
        WetTool::Erase,
        0,
        220.0,
        300.0,
        256.0,
        0,
    )]));
    c.idle(60);
    let img = c.snapshot().unwrap();
    let png = save(&img, "erase");

    let erased = img.mean_rgb(250, 250, 270, 262);
    let kept = img.mean_rgb(120, 250, 180, 262);
    let ctx = format!(
        "{}\n消去部={erased:?} 残り={kept:?} 紙={paper:?}",
        png.display()
    );
    assert!(
        dist(erased, paper) < 0.02,
        "消去部が紙の白に戻っていない\n{ctx}"
    );
    assert!(hsv(kept).1 > 0.2, "消していない部分まで色が抜けた\n{ctx}");
}

/// 削り(リフト → 吸い取り → 乾燥)で薄くなった割合(0 = 変化なし、1 = 紙の白まで)
fn lift_ratio(channel: u32, name: &str) -> (f32, String) {
    let mut c = canvas();
    let paper = lightness(paper(&c));
    c.play(recording(vec![hstroke(
        WetTool::Paint,
        channel,
        100.0,
        400.0,
        256.0,
        0,
    )]));
    c.idle(120);
    c.fast_dry().unwrap();
    let before = c.snapshot().unwrap();
    c.play(recording(vec![hstroke(
        WetTool::Lift,
        channel,
        220.0,
        300.0,
        256.0,
        0,
    )]));
    c.idle(10);
    c.play(recording(vec![hstroke(
        WetTool::Absorb,
        channel,
        220.0,
        300.0,
        256.0,
        0,
    )]));
    c.idle(10);
    c.fast_dry().unwrap();
    let img = c.snapshot().unwrap();
    let png = save(&img, name);

    let b = lightness(before.mean_rgb(250, 250, 270, 262));
    let a = lightness(img.mean_rgb(250, 250, 270, 262));
    let kept_before = before.mean_rgb(120, 250, 180, 262);
    let kept_after = img.mean_rgb(120, 250, 180, 262);
    let ctx = format!(
        "{}\n明度 前={b:.3} 後={a:.3} 紙={paper:.3} 範囲外 前={kept_before:?} 後={kept_after:?}",
        png.display()
    );
    assert!(
        dist(kept_before, kept_after) < 0.03,
        "削っていない部分まで変わった\n{ctx}"
    );
    ((a - b) / (paper - b), ctx)
}

/// 削り: 乾いた色を水で浮かせて吸い取ると薄くなる
#[test]
fn lift_lightens_dried_paint() {
    let (ratio, ctx) = lift_ratio(SIENNA, "lift_sienna");
    assert!(
        ratio > 0.3,
        "削っても薄くならない(回復率 {ratio:.2})\n{ctx}"
    );
}

/// 顔料個性: 染みつき(ω)の強いフタロブルーは、弱いバーントシェンナより削れ残る
#[test]
fn staining_pigment_resists_lift() {
    let (sienna, ctx_s) = lift_ratio(SIENNA, "lift_sienna_cmp");
    let (blue, ctx_b) = lift_ratio(BLUE, "lift_blue");
    assert!(
        blue < sienna - 0.1,
        "染みつきの差が削れ方に出ていない(青 {blue:.2} / シェンナ {sienna:.2})\n{ctx_b}\n{ctx_s}"
    );
}

/// 同梱の記録ストローク(assets/strokes/*.json)がすべて再生でき、何かが描かれる
#[test]
fn recorded_strokes_replay() {
    for name in kasaneiro::replay::list() {
        let mut c = canvas();
        let paper = paper(&c);
        let stored = kasaneiro::replay::load(&name).unwrap();
        c.set_palette(stored.palette);
        c.play(stored.recording);
        c.idle(60);
        let img = c.snapshot().unwrap();
        let png = save(&img, &format!("stroke-{name}"));
        let total = (img.size * img.size) as usize;
        let painted = (0..img.size)
            .flat_map(|y| (0..img.size).map(move |x| (x, y)))
            .filter(|&(x, y)| dist(img.rgb(x, y), paper) > 0.05)
            .count();
        assert!(
            painted > total / 200 && painted < total,
            "{name}: 描かれた画素が {painted}/{total}({})",
            png.display()
        );
    }
}

/// アクティブタイル: 計算しないタイルの書き込みを省いても、全面計算(active_tiles=0)と同じ絵になる。
/// 乾かす(bake)・Fast Dry で全面が書き替わった後に ping-pong の2枚がずれていれば、
/// 古い側が表に出て差が出る
#[test]
fn active_tiles_match_full_compute() {
    let run = |active_tiles: u32| {
        let mut c = canvas();
        c.params.active_tiles = active_tiles;
        c.play(recording(vec![hstroke(WetTool::Paint, YELLOW, 80.0, 220.0, 120.0, 0)]));
        c.idle(30);
        c.fast_dry().unwrap();
        c.play(recording(vec![hstroke(WetTool::Paint, BLUE, 260.0, 420.0, 260.0, 0)]));
        c.idle(30);
        c.bake_dry().unwrap();
        c.idle(3);
        c.play(recording(vec![hstroke(WetTool::Paint, SIENNA, 80.0, 420.0, 400.0, 0)]));
        c.idle(30);
        c.snapshot().unwrap()
    };
    let tiled = run(1);
    let full = run(0);
    let png = save(&tiled, "active_tiles_tiled");
    save(&full, "active_tiles_full");
    let max_diff = tiled
        .rgba
        .iter()
        .zip(&full.rgba)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(
        max_diff <= 2,
        "アクティブタイルの有無で絵が変わる(最大差 {max_diff}/255)\n{}",
        png.display()
    );
}

/// アクティブタイル: 計算から外れたタイル(書き込みを省くタイル)では ping-pong の2枚が一致している。
/// 濡れマスクは蒸発では消えないので、タイルが計算から外れるのは Fast Dry・乾かす(bake)の後。
/// これらは current 側だけを書き替えるので、片方が古いまま残ると後でそのタイルを読むパスに混ざる
#[test]
fn inactive_tiles_keep_pingpong_in_sync() {
    let mut c = canvas();
    c.play(recording(vec![hstroke(WetTool::Paint, YELLOW, 80.0, 220.0, 120.0, 0)]));
    c.idle(10);
    assert_ne!(c.pingpong_mismatch().unwrap(), [0; 3], "濡れている間は2枚が食い違うはず");
    c.fast_dry().unwrap();
    c.idle(2);
    assert_eq!(c.pingpong_mismatch().unwrap(), [0; 3], "Fast Dry の後に2枚がずれている");

    c.play(recording(vec![hstroke(WetTool::Paint, BLUE, 260.0, 420.0, 260.0, 0)]));
    c.idle(10);
    c.bake_dry().unwrap();
    c.idle(2);
    assert_eq!(c.pingpong_mismatch().unwrap(), [0; 3], "乾かした後に2枚がずれている");
}
