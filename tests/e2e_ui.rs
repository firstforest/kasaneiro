//! UI 込みの e2e: アプリ全体(PaintApp)を egui_kittest でウィンドウなしに起動し、
//! 実際のポインタ入力・クリックで操作して画面(wgpu で描画した画像)と UI の状態を確かめる。
//! 要素は AccessKit の名前(ボタンの文字、キャンバス・色スウォッチに付けた名前)で引く。
//! 描いた結果の画像は target/tmp/e2e/ui-*.png に書き出す。

mod common;

use common::*;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, wgpu::WgpuTestRenderer};
use kasaneiro::app::PaintApp;
use std::path::PathBuf;

fn app() -> Harness<'static, PaintApp> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 780.0))
        // 実機と同じ 60fps のテンポ(既定 0.25 秒/フレームではダブルクリック判定 0.3 秒を超え、
        // 筆の含み(stable_dt で減る)も実機と違う速さで尽きる)
        .with_step_dt(1.0 / 60.0)
        .renderer(WgpuTestRenderer::from_render_state(render_state()))
        .build_eframe(|cc| PaintApp::new(cc));
    frames(&mut harness, 3);
    harness
}

/// 1フレーム進めて描画する。キャンバスのシミュは描画時(ペイントコールバックの prepare)に
/// 進むので、step() だけだとそのフレームの筆の入力が GPU に届かない
fn frame(harness: &mut Harness<'_, PaintApp>) {
    harness.step();
    harness.render().expect("描画");
}

/// `n` フレーム進める。常時再描画するアプリなので run()(収束待ち)は使わない
fn frames(harness: &mut Harness<'_, PaintApp>, n: usize) {
    for _ in 0..n {
        frame(harness);
    }
}

/// キャンバスの画面上の矩形(論理座標)
fn canvas_rect(harness: &Harness<'_, PaintApp>) -> egui::Rect {
    harness.get_by_label("キャンバス").rect()
}

/// 画面を描画して、キャンバス矩形内の点(0..1 の相対位置)の色を読む
fn canvas_color(
    harness: &mut Harness<'_, PaintApp>,
    name: &str,
    at: &[(f32, f32)],
) -> Vec<[f32; 3]> {
    let rect = canvas_rect(harness);
    let ppp = harness.ctx.pixels_per_point();
    let shot = harness.render().expect("描画");
    let png = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("e2e")
        .join(format!("ui-{name}.png"));
    std::fs::create_dir_all(png.parent().unwrap()).unwrap();
    shot.save(&png).unwrap();
    at.iter()
        .map(|&(u, v)| {
            let p = rect.lerp_inside(egui::vec2(u, v)) * ppp;
            let px = shot.get_pixel(p.x as u32, p.y as u32);
            [0, 1, 2].map(|c| px[c] as f32 / 255.0)
        })
        .collect()
}

/// マウスでキャンバス上を (u0,v)→(u1,v) へドラッグする(相対位置)
fn mouse_drag(harness: &mut Harness<'_, PaintApp>, u0: f32, u1: f32, v: f32) {
    let rect = canvas_rect(harness);
    let at = |u: f32| rect.lerp_inside(egui::vec2(u, v));
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    harness.event(egui::Event::PointerMoved(at(u0)));
    harness.event(button(at(u0), true));
    frame(harness);
    for i in 1..=20 {
        let u = u0 + (u1 - u0) * i as f32 / 20.0;
        harness.event(egui::Event::PointerMoved(at(u)));
        frame(harness);
    }
    harness.event(button(at(u1), false));
    frame(harness);
}

/// ペン(筆圧付き Touch)でキャンバス上を (u0,v)→(u1,v) へ引く
fn pen_stroke(harness: &mut Harness<'_, PaintApp>, u0: f32, u1: f32, v: f32, force: f32) {
    let rect = canvas_rect(harness);
    let touch = |u: f32, phase| egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase,
        pos: rect.lerp_inside(egui::vec2(u, v)),
        force: Some(force),
    };
    harness.event(touch(u0, egui::TouchPhase::Start));
    frame(harness);
    for i in 1..=20 {
        let u = u0 + (u1 - u0) * i as f32 / 20.0;
        harness.event(touch(u, egui::TouchPhase::Move));
        frame(harness);
    }
    harness.event(touch(u1, egui::TouchPhase::End));
    frame(harness);
}

/// 起動するとシェーダーが通り、白紙のキャンバスが表示される
#[test]
fn starts_with_blank_canvas() {
    let mut harness = app();
    let colors = canvas_color(&mut harness, "start", &[(0.5, 0.5), (0.2, 0.8)]);
    for c in &colors {
        assert!(
            lightness(*c) > 0.9 && hsv(*c).1 < 0.1,
            "紙の白でない: {colors:?}"
        );
    }
}

/// マウスでドラッグすると、その線の上にだけ色が乗る(入力 → splat → シミュ → 表示の一周)
#[test]
fn mouse_drag_paints_on_canvas() {
    let mut harness = app();
    harness.get_by_label_contains("塗る色 2:").click(); // フタロブルー
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.3, 0.7, 0.5);
    frames(&mut harness, 30);
    let colors = canvas_color(&mut harness, "mouse", &[(0.5, 0.5), (0.5, 0.15)]);
    let (stroke, away) = (colors[0], colors[1]);
    let (h, s, _) = hsv(stroke);
    assert!(
        (170.0..260.0).contains(&h) && s > 0.15,
        "線の上が青くない: {stroke:?} {:?}",
        hsv(stroke)
    );
    assert!(
        lightness(away) > 0.9,
        "線から離れた場所まで色が付いた: {away:?}"
    );
}

/// ペンの筆圧が線の濃さ・太さに効く(弱い筆圧の線は強い筆圧の線より細く薄い)
#[test]
fn pen_pressure_changes_stroke() {
    let mut harness = app();
    harness.get_by_label_contains("塗る色 2:").click();
    frames(&mut harness, 2);
    pen_stroke(&mut harness, 0.2, 0.8, 0.3, 1.0);
    frames(&mut harness, 5);
    pen_stroke(&mut harness, 0.2, 0.8, 0.7, 0.15);
    frames(&mut harness, 60);
    // 線の中心と、中心から少し外した位置(太さの差が出る)
    let colors = canvas_color(
        &mut harness,
        "pen",
        &[(0.5, 0.3), (0.5, 0.7), (0.5, 0.33), (0.5, 0.73)],
    );
    let ink = |c: [f32; 3]| 1.0 - lightness(c);
    assert!(
        ink(colors[0]) > ink(colors[1]) && ink(colors[2]) > ink(colors[3]),
        "筆圧の差が線に出ていない: 強 中心={:?} 外={:?} / 弱 中心={:?} 外={:?}",
        colors[0],
        colors[2],
        colors[1],
        colors[3]
    );
}

/// ツールボタンを押すと選択中ツールが切り替わり、説明の1行も変わる
#[test]
fn tool_button_switches_tool() {
    let mut harness = app();
    assert!(
        harness
            .query_by_label("水と顔料を置いて描く基本のブラシ")
            .is_some()
    );
    harness.get_by_label("削り").click();
    frames(&mut harness, 2);
    assert!(
        harness
            .query_by_label("乾いた色を水で戻して薄くする削りツール")
            .is_some()
    );
    assert!(
        harness
            .query_by_label("水と顔料を置いて描く基本のブラシ")
            .is_none()
    );
}

/// 色スウォッチのダブルクリックで、その色の「色をつくる」モーダルが開き、閉じるで閉じる
#[test]
fn double_click_swatch_opens_color_modal() {
    let mut harness = app();
    assert!(harness.query_by_label("色をつくる").is_none());
    let swatch = harness.get_by_label_contains("塗る色 3:").rect().center();
    harness.event(egui::Event::PointerMoved(swatch));
    frame(&mut harness);
    // 押す・離すを1フレームずつ(実際の操作と同じく、ダブルクリック判定の時間内に収まる)
    for pressed in [true, false, true, false] {
        harness.event(egui::Event::PointerButton {
            pos: swatch,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        frame(&mut harness);
    }
    frames(&mut harness, 2);
    assert!(
        harness.query_by_label("色をつくる").is_some(),
        "モーダルが開かない"
    );
    // 編集対象はダブルクリックしたスロット(#3)
    assert!(
        harness.query_by_label("#3").is_some(),
        "編集対象が #3 でない"
    );

    harness.get_by_label("閉じる").click();
    frames(&mut harness, 3);
    assert!(harness.query_by_label("色をつくる").is_none(), "閉じない");
}

/// 「乾かす(固定)」で乾燥レイヤーに焼き込んでも色は残る。濡れ色(水量で最大 12% 暗く
/// 表示。display.wgsl)が抜けて少し明るくなるだけで、色相は変わらない
#[test]
fn dry_keeps_painted_color() {
    let mut harness = app();
    mouse_drag(&mut harness, 0.3, 0.7, 0.5);
    frames(&mut harness, 30);
    let before = canvas_color(&mut harness, "dry-before", &[(0.5, 0.5)])[0];
    harness.get_by_label("乾かす(固定)").click();
    frames(&mut harness, 5);
    let after = canvas_color(&mut harness, "dry-after", &[(0.5, 0.5)])[0];
    let ctx = format!(
        "前={before:?} {:?} 後={after:?} {:?}",
        hsv(before),
        hsv(after)
    );
    assert!(lightness(before) < 0.9, "そもそも描けていない: {ctx}");
    assert!(
        (hsv(before).0 - hsv(after).0).abs() < 10.0,
        "乾かしたら色相が変わった: {ctx}"
    );
    assert!(hsv(after).1 > 0.3, "乾かしたら色が抜けた: {ctx}");
    assert!(
        lightness(after) >= lightness(before) && lightness(after) < lightness(before) / 0.88 + 0.02,
        "乾いて明るくなる幅が濡れ色の分を超えている: {ctx}"
    );
}
