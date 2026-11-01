//! 水彩シミュの e2e: ストロークを headless で再生し、3つのコア要件(wet-on-wet / グレージング /
//! 削り)を見た目の指標で判定する。
//!
//! シェーダーの挙動を細かく固定するテストではなく、「目で見て判定する体験」(docs/design.md §1 の設計原則)が
//! 壊れていないかを色相・明度の帯で見る回帰検知。パラメータ調整で色味が多少動いても通る幅にしてある。
//! 結果画像は target/tmp/e2e/*.png に書き出すので、落ちたらまず画像を見る。
//! 既定パレット: 0=ハンザイエロー / 1=フタロブルー / 2=キナクリドンマゼンタ / 3=バーントシェンナ

mod common;

use common::*;
use kasaneiro::headless::{HeadlessCanvas, Image};
use paint_core::load::BrushLoad;
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
            // 受け渡しの式では筆跡の重なりで顔料が積み上がらず全体が淡いので、彩度は控えめに見る
            GREEN_HUE.contains(&h) && s > 0.05
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

/// 下地への統合: 下の乾燥レイヤーを焼き固めても見た目(乗算・KM の両方式)は変わらず、
/// 空いた枠にまた乾かせる
#[test]
fn merge_into_base_keeps_look() {
    let mut c = canvas();
    // 横・縦・斜めに交差させて、重ね順が効く KM 合成の交差部も比べる
    let strokes = [
        (YELLOW, [100.0, 250.0], [400.0, 250.0]),
        (BLUE, [250.0, 100.0], [250.0, 400.0]),
        (SIENNA, [120.0, 120.0], [380.0, 380.0]),
    ];
    for (channel, a, b) in strokes {
        c.play(recording(vec![stroke(WetTool::Paint, channel, a, b, 0)]));
        c.idle(120);
        c.bake_dry().unwrap();
    }
    let look = |c: &mut HeadlessCanvas| {
        [0, 1].map(|mode| {
            c.params.compose_mode = mode;
            c.idle(1); // 合成キャッシュを焼き直すフレーム
            c.snapshot().unwrap()
        })
    };
    let before = look(&mut c);
    c.merge_into_base(2).unwrap();
    assert_eq!(c.layer_count(), 1);
    let after = look(&mut c);
    let png = save(&after[1], "merge_into_base");

    for mode in 0..2 {
        let worst = before[mode]
            .rgba
            .iter()
            .zip(&after[mode].rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        // 下地は合成キャッシュと同じ半精度なので、丸めの差が 8bit で数段出る程度に収まる
        assert!(
            worst <= 2,
            "統合で見た目が変わった(合成方式 {mode}、最大差 {worst})\n{}",
            png.display()
        );
    }

    // 空いた枠にまた乾かせて、下地の上に重なる
    c.play(recording(vec![hstroke(
        WetTool::Paint,
        BLUE,
        100.0,
        400.0,
        450.0,
        0,
    )]));
    c.idle(120);
    c.bake_dry().unwrap();
    assert_eq!(c.layer_count(), 2);
}

/// 乾いた層の濃度倍率: 倍率を下げると薄く、上げると濃くなり、0 で紙に戻り、1 に戻せば元の見た目になる
#[test]
fn dried_layer_density_scales_look() {
    let mut c = canvas();
    c.play(recording(vec![hstroke(
        WetTool::Paint,
        BLUE,
        100.0,
        400.0,
        250.0,
        0,
    )]));
    c.idle(120);
    c.bake_dry().unwrap();
    let look = |c: &mut HeadlessCanvas, density: f32| {
        c.set_layer_density(0, density);
        c.idle(1); // 合成キャッシュを焼き直すフレーム
        c.snapshot().unwrap()
    };
    let base = look(&mut c, 1.0);
    let light = look(&mut c, 0.4);
    let dark = look(&mut c, 2.0);
    let none = look(&mut c, 0.0);
    let back = look(&mut c, 1.0);
    let png = save(&light, "dried_layer_density");

    let sum = |rgb: [f32; 3]| rgb.iter().sum::<f32>();
    let at = |img: &Image| sum(img.mean_rgb(200, 245, 300, 255));
    let paper = sum(base.mean_rgb(200, 400, 300, 410));
    let ctx = format!(
        "{}\n倍率1={} 0.4={} 2={} 0={} 紙={paper}",
        png.display(),
        at(&base),
        at(&light),
        at(&dark),
        at(&none)
    );
    assert!(
        at(&dark) < at(&base) && at(&base) < at(&light) && at(&light) < paper,
        "濃さが倍率の順に並ばない\n{ctx}"
    );
    assert!(
        (at(&none) - paper).abs() < 0.02,
        "倍率 0 で紙に戻らない\n{ctx}"
    );
    assert!(
        base.rgba == back.rgba,
        "倍率を 1 に戻しても元の見た目にならない\n{ctx}"
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
        c.play(recording(vec![hstroke(
            WetTool::Paint,
            YELLOW,
            80.0,
            220.0,
            120.0,
            0,
        )]));
        c.idle(30);
        c.fast_dry().unwrap();
        c.play(recording(vec![hstroke(
            WetTool::Paint,
            BLUE,
            260.0,
            420.0,
            260.0,
            0,
        )]));
        c.idle(30);
        c.bake_dry().unwrap();
        c.idle(3);
        c.play(recording(vec![hstroke(
            WetTool::Paint,
            SIENNA,
            80.0,
            420.0,
            400.0,
            0,
        )]));
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

/// アクティブタイル: 蒸発で水が尽きた領域(濡れマスクは残る)は計算から外れるが、その隣に
/// 新しく水を置いて湿った紙を伝って広がる場合も、全面計算と同じ絵になる
#[test]
fn active_tiles_match_full_compute_after_evaporation() {
    let run = |active_tiles: u32| {
        let mut c = canvas();
        c.params.active_tiles = active_tiles;
        // 蒸発を速めて、水が尽きるまでを短く回す
        c.params.evap_rate = 0.02;
        c.play(recording(vec![hstroke(
            WetTool::Paint,
            YELLOW,
            80.0,
            420.0,
            200.0,
            0,
        )]));
        c.idle(200);
        c.params.evap_rate = 0.0003;
        c.play(recording(vec![stroke(
            WetTool::Paint,
            BLUE,
            [250.0, 120.0],
            [250.0, 260.0],
            0,
        )]));
        c.idle(60);
        c.snapshot().unwrap()
    };
    let tiled = run(1);
    let full = run(0);
    let png = save(&tiled, "active_tiles_evaporated_tiled");
    save(&full, "active_tiles_evaporated_full");
    let max_diff = tiled
        .rgba
        .iter()
        .zip(&full.rgba)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(
        max_diff <= 2,
        "蒸発後のアクティブタイルの有無で絵が変わる(最大差 {max_diff}/255)\n{}",
        png.display()
    );
}

/// アクティブタイル: 計算から外れたタイル(書き込みを省くタイル)では ping-pong の2枚が一致している。
/// Fast Dry・乾かす(bake)は current 側だけを書き替えるので、片方が古いまま残ると
/// 後でそのタイルを読むパスに混ざる
#[test]
fn inactive_tiles_keep_pingpong_in_sync() {
    let mut c = canvas();
    c.play(recording(vec![hstroke(
        WetTool::Paint,
        YELLOW,
        80.0,
        220.0,
        120.0,
        0,
    )]));
    c.idle(10);
    assert_ne!(
        c.pingpong_mismatch().unwrap(),
        [0; 3],
        "濡れている間は2枚が食い違うはず"
    );
    c.fast_dry().unwrap();
    c.idle(2);
    assert_eq!(
        c.pingpong_mismatch().unwrap(),
        [0; 3],
        "Fast Dry の後に2枚がずれている"
    );

    c.play(recording(vec![hstroke(
        WetTool::Paint,
        BLUE,
        260.0,
        420.0,
        260.0,
        0,
    )]));
    c.idle(10);
    c.bake_dry().unwrap();
    c.idle(2);
    assert_eq!(
        c.pingpong_mismatch().unwrap(),
        [0; 3],
        "乾かした後に2枚がずれている"
    );
}

/// 塗る筆の吸い込み: 濡れた黄の上を、乾きかけの透明な筆でなでて乾いた紙まで引くと、
/// 筆が水と黄を吸って黄の筆になり、その先の紙へ黄を運ぶ。溜まりのない紙で同じ筆を引いたものと比べる
#[test]
fn paint_brush_picks_up_wet_color() {
    let run = |pool: bool| {
        let mut c = canvas();
        if pool {
            // 濡れた黄の溜まり(3本並べて太くする)
            let strokes = [230.0, 250.0, 270.0]
                .iter()
                .enumerate()
                .map(|(k, &y)| hstroke(WetTool::Paint, YELLOW, 60.0, 220.0, y, k as u32 * 40))
                .collect();
            c.play(recording(strokes));
            c.idle(10);
        }
        // 左から右の乾いた紙へ、水が 5% だけ残った透明な筆で引く
        let mut thirsty = hstroke(WetTool::Paint, YELLOW, 80.0, 440.0, 250.0, 0);
        thirsty.load = Some(BrushLoad {
            water: 0.05,
            pigment: [0.0; 4],
            tip: None,
        });
        c.play(recording(vec![thirsty]));
        let brush = c.brush_load().unwrap();
        c.idle(120);
        (c.snapshot().unwrap(), brush)
    };
    let (img, brush) = run(true);
    let (control, control_brush) = run(false);
    let png = save(&img, "brush_pickup");
    save(&control, "brush_pickup_off");
    // 黄の溜まりを抜けた先(乾いた紙だった所)の線
    let trail = img.mean_rgb(300, 244, 420, 256);
    let control_trail = control.mean_rgb(300, 244, 420, 256);
    let ctx = format!(
        "{}
溜まりあり: 線 {:?} hsv {:?} 筆 {:?}
溜まりなし: 線 {:?} hsv {:?} 筆 {:?}",
        png.display(),
        trail,
        hsv(trail),
        brush,
        control_trail,
        hsv(control_trail),
        control_brush
    );

    // 溜まりは標準の濃さ(濃さ 1)なので、吸った筆はその色に寄り、水も増える
    assert!(
        brush.concentration() > 0.3 && brush.tip().water > 0.15,
        "筆が黄の溜まりを吸い込んでいない
{ctx}"
    );
    assert_eq!(
        control_brush.pigment, [0.0; 4],
        "透明な筆に色が入った
{ctx}"
    );
    let (h, s, _) = hsv(trail);
    assert!(
        (40.0..80.0).contains(&h) && s > hsv(control_trail).1 + 0.03,
        "吸い込んだ黄が先の紙へ運ばれていない
{ctx}"
    );
}

/// 満タンの筆は乾いた紙で減り(腹の大きさ brush_hold の見積もりより少し長持ちする。穂先の濡れ具合の分だけ
/// 出る量が少ないため)、ふつうの速さなら穂先は腹からの補充で乾かない。自分が塗ったばかりの所を
/// 戻っても、行きより減りがずっと小さい(紙と穂先の濡れ具合がほぼ釣り合っている)
#[test]
fn full_brush_depletes_on_dry_paper_only() {
    let mut c = canvas();
    c.play(recording(vec![hstroke(
        WetTool::Paint,
        BLUE,
        100.0,
        400.0,
        250.0,
        0,
    )]));
    let there = c.brush_load().unwrap();
    let mut back = hstroke(WetTool::Paint, BLUE, 400.0, 100.0, 250.0, 0);
    back.load = Some(there);
    c.play(recording(vec![back]));
    let back = c.brush_load().unwrap();
    // 腹だけで 300px 塗ったときの見積もり 0.5^(300 / brush_hold)
    let expected = 0.5f32.powf(300.0 / c.params.brush_hold);
    let ctx = format!("行き {there:?} / 戻り {back:?}(腹の見積もり {expected:.2})");
    assert!(
        (expected - 0.02..0.97).contains(&there.water),
        "乾いた紙で筆が見込みどおり減っていない: {ctx}"
    );
    assert!(
        there.tip().water > 0.85 * there.water,
        "ふつうの速さで穂先が乾いた(腹からの補充が追いつかない): {ctx}"
    );
    // 戻りで減るのは、行きで減った量の半分以下(紙が穂先とほぼ同じだけ濡れているので受け渡しが小さい)
    assert!(
        back.water <= there.water + 0.01 && there.water - back.water < 0.5 * (1.0 - there.water),
        "自分が塗った所を戻って筆が大きく変わった: {ctx}"
    );
    assert!(
        (there.concentration() - 1.0).abs() < 0.1 && (back.concentration() - 1.0).abs() < 0.1,
        "筆の濃さが変わった: {ctx}"
    );
}

/// 洗った空の筆: 乾いた紙では何も置かず、濡れた絵の具の中をなでると穂先が色水を吸い取る
/// (吸った跡は溜まりより淡くなる)
#[test]
fn washed_brush_lifts_wet_paint() {
    let mut c = canvas();
    // 乾いた紙を空の筆でなでても何も起きない
    let mut dry = hstroke(WetTool::Paint, YELLOW, 60.0, 450.0, 400.0, 0);
    dry.load = Some(BrushLoad::empty());
    c.play(recording(vec![dry]));
    let after_dry = c.brush_load().unwrap();
    // 濡れた黄の溜まり(3本並べて太くする)の中を、空の筆で横切る
    let pool = [230.0, 250.0, 270.0]
        .iter()
        .enumerate()
        .map(|(k, &y)| hstroke(WetTool::Paint, YELLOW, 60.0, 450.0, y, k as u32 * 80))
        .collect();
    c.play(recording(pool));
    c.idle(10);
    let before = c.snapshot().unwrap().mean_rgb(150, 245, 350, 255);
    let mut washed = hstroke(WetTool::Paint, YELLOW, 80.0, 430.0, 250.0, 0);
    washed.load = Some(BrushLoad::empty());
    c.play(recording(vec![washed]));
    let brush = c.brush_load().unwrap();
    c.idle(60);
    let img = c.snapshot().unwrap();
    let png = save(&img, "washed_brush_lift");
    let lifted = img.mean_rgb(150, 245, 350, 255);
    let blank = img.mean_rgb(100, 395, 400, 405);
    let ctx = format!(
        "{}\n乾いた紙の後の筆 {after_dry:?}\n吸った後の筆 {brush:?}\n溜まり {before:?} → 吸った跡 {lifted:?} / 乾いた紙をなでた所 {blank:?}",
        png.display()
    );

    assert_eq!(
        after_dry,
        BrushLoad::empty(),
        "乾いた紙で空の筆が変わった\n{ctx}"
    );
    assert!(
        hsv(blank).1 < 0.03 && lightness(blank) > lightness(paper(&c)) - 0.02,
        "空の筆が乾いた紙に何か置いた\n{ctx}"
    );
    assert!(
        brush.tip().water > 0.1 && brush.tip().pigment[YELLOW as usize] > 0.05,
        "空の筆が濡れた黄を吸っていない\n{ctx}"
    );
    assert!(
        hsv(lifted).1 < hsv(before).1 - 0.01,
        "吸った跡が淡くなっていない\n{ctx}"
    );
}
