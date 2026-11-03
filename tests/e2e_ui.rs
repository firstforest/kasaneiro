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

/// 筆の中身で濃さを描き分けられる: 水に浸してから青を取った線 > 水に浸して薄めた線 > 洗って水だけにした筆の線(ほぼ紙の白)。
/// 色を取っても水は変わらないので、起動時の空の筆はまず水に浸す
#[test]
fn brush_load_controls_density() {
    let mut harness = app();
    harness.get_by_label("水").click();
    harness.get_by_label_contains("塗る色 2:").click();
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.2, 0.8, 0.2);
    harness.get_by_label("水").click();
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.2, 0.8, 0.5);
    harness.get_by_label("洗う").click();
    harness.get_by_label("水").click();
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.2, 0.8, 0.8);
    frames(&mut harness, 60);
    let colors = canvas_color(
        &mut harness,
        "density",
        &[(0.5, 0.2), (0.5, 0.5), (0.5, 0.8)],
    );
    let ink = |c: [f32; 3]| 1.0 - lightness(c);
    let ctx = format!(
        "濃 {:?} / 薄 {:?} / 水 {:?}",
        colors[0], colors[1], colors[2]
    );
    assert!(
        ink(colors[0]) > ink(colors[1]) + 0.03,
        "水で薄まっていない: {ctx}"
    );
    assert!(
        ink(colors[1]) > ink(colors[2]) + 0.03,
        "薄めた線が水の線と変わらない: {ctx}"
    );
    assert!(
        hsv(colors[2]).1 < 0.1,
        "洗って水だけにした筆で色が乗った: {ctx}"
    );
}

/// 色スウォッチの押下は今の色に足す(筆の中で混ざる)、Shift+押下は持ち替え(筆の顔料を捨ててその色だけにする)。
/// 黄 → Shift+青と押した筆の線は青のまま、そこへ黄を足した筆の線は緑寄りになる
#[test]
fn swatch_mixes_and_shift_switches() {
    let mut harness = app();
    harness.get_by_label("水").click();
    harness.get_by_label_contains("塗る色 1:").click(); // ハンザイエロー
    harness
        .get_by_label_contains("塗る色 2:")
        .click_modifiers(egui::Modifiers::SHIFT); // フタロブルーに持ち替え
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.2, 0.8, 0.3);
    harness.get_by_label_contains("塗る色 1:").click(); // 黄を足す
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.2, 0.8, 0.7);
    frames(&mut harness, 60);
    let colors = canvas_color(&mut harness, "switch-mix", &[(0.5, 0.3), (0.5, 0.7)]);
    let (switched, mixed) = (hsv(colors[0]), hsv(colors[1]));
    let ctx = format!(
        "持ち替え {:?} {switched:?} / 足す {:?} {mixed:?}",
        colors[0], colors[1]
    );
    assert!(
        (190.0..260.0).contains(&switched.0) && switched.1 > 0.15,
        "持ち替えた筆に前の黄が残っている: {ctx}"
    );
    assert!(
        mixed.0 < switched.0 - 15.0 && mixed.1 > 0.15,
        "足した黄が混ざっていない: {ctx}"
    );
}

/// スポイトは筆に顔料を取る: 黄+青で塗った緑の所を拾うと、洗った筆が同じ緑で塗れる。
/// パレットは変わらない(拾った後に Shift で持ち替えた黄スウォッチの線は黄のまま)
#[test]
fn eyedropper_copies_pigment_to_brush() {
    let mut harness = app();
    harness.get_by_label_contains("塗る色 1:").click(); // ハンザイエロー
    harness.get_by_label_contains("塗る色 2:").click(); // フタロブルーを足す
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.2, 0.8, 0.2);
    frames(&mut harness, 30);

    harness.get_by_label("洗う").click();
    harness.get_by_label("💧 スポイト").click();
    frames(&mut harness, 2);
    let at = canvas_rect(&harness).lerp_inside(egui::vec2(0.5, 0.2));
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    harness.event(egui::Event::PointerMoved(at));
    harness.event(button(true));
    frame(&mut harness);
    harness.event(button(false));
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.2, 0.8, 0.5);

    harness
        .get_by_label_contains("塗る色 1:")
        .click_modifiers(egui::Modifiers::SHIFT);
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.2, 0.8, 0.8);
    frames(&mut harness, 60);

    let colors = canvas_color(
        &mut harness,
        "eyedropper",
        &[(0.5, 0.2), (0.5, 0.5), (0.5, 0.8)],
    );
    let (source, picked, yellow) = (hsv(colors[0]), hsv(colors[1]), hsv(colors[2]));
    let ctx = format!(
        "元 {:?} {source:?} / 拾った筆 {:?} {picked:?} / 黄 {:?} {yellow:?}",
        colors[0], colors[1], colors[2]
    );
    assert!(picked.1 > 0.15, "スポイトの筆で色が乗らない: {ctx}");
    assert!(
        (picked.0 - source.0).abs() < 20.0,
        "拾った筆の色相が元の所と違う: {ctx}"
    );
    assert!(
        (30.0..75.0).contains(&yellow.0) && yellow.1 > 0.15,
        "スポイトの後に黄スウォッチが黄で塗れない(パレットが変わった?): {ctx}"
    );
}

/// ペンの筆圧が線の太さに効く(弱い筆圧の線は強い筆圧の線より細い)。濃さは筆圧でほぼ変わらず、
/// 線の中心の1点は混ざりムラ(mix_mottle)で揺れるので比べない
#[test]
fn pen_pressure_changes_stroke() {
    let mut harness = app();
    harness.get_by_label_contains("塗る色 2:").click();
    frames(&mut harness, 2);
    pen_stroke(&mut harness, 0.2, 0.8, 0.3, 1.0);
    frames(&mut harness, 5);
    pen_stroke(&mut harness, 0.2, 0.8, 0.7, 0.15);
    frames(&mut harness, 60);
    // 中心から少し外した位置: 強い線の幅の内側で、弱い線の幅の外側
    let colors = canvas_color(&mut harness, "pen", &[(0.5, 0.33), (0.5, 0.73)]);
    let ink = |c: [f32; 3]| 1.0 - lightness(c);
    assert!(
        ink(colors[0]) > ink(colors[1]) + 0.1,
        "筆圧の差が線の太さに出ていない: 強={:?} 弱={:?}",
        colors[0],
        colors[1]
    );
}

/// 勢いよく払ってペンが離れるとき、winit は離れる直前にまとめて届いた位置を筆圧 0 で送り、
/// 筆圧 0 は `force: None` になる。それを筆圧のない入力(1.0)と取り違えず、末尾が太く丸くならない
#[test]
fn pen_lift_without_force_does_not_fatten_tail() {
    let mut harness = app();
    harness.get_by_label_contains("塗る色 2:").click();
    frames(&mut harness, 2);
    let rect = canvas_rect(&harness);
    let touch = |u: f32, phase, force| egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase,
        pos: rect.lerp_inside(egui::vec2(u, 0.5)),
        force,
    };
    harness.event(touch(0.2, egui::TouchPhase::Start, Some(0.15)));
    frame(&mut harness);
    for i in 1..=10 {
        let u = 0.2 + 0.04 * i as f32;
        harness.event(touch(u, egui::TouchPhase::Move, Some(0.15)));
        frame(&mut harness);
    }
    // 離れる瞬間: 直前の位置が筆圧なしで届き、続けて End(実機では1フレームにまとめて届くが、
    // kittest は1フレームに積んだイベントを別々のパスで処理し、描かれないパスの splat が捨てられる)
    for u in [0.65, 0.7, 0.75, 0.8] {
        harness.event(touch(u, egui::TouchPhase::Move, None));
        frame(&mut harness);
    }
    harness.event(touch(0.8, egui::TouchPhase::End, None));
    frame(&mut harness);
    frames(&mut harness, 60);
    // 線の中心から外した位置: 筆圧 1.0 の太さなら塗られ、筆圧 0.15 の細さなら紙のまま
    let colors = canvas_color(&mut harness, "pen-lift", &[(0.4, 0.53), (0.72, 0.53)]);
    let ink = |c: [f32; 3]| 1.0 - lightness(c);
    assert!(
        ink(colors[1]) < ink(colors[0]) + 0.05,
        "ペンを離した所が太くなっている: 胴={:?} 末尾={:?}",
        colors[0],
        colors[1]
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
            .query_by_label("濡らして押さえ、乾いた色を抜いて白に近づける")
            .is_some()
    );
    assert!(
        harness
            .query_by_label("水と顔料を置いて描く基本のブラシ")
            .is_none()
    );
}

/// 「色をつくる」ボタンで選択中スロット(最後に押した色スウォッチ)の編集モーダルが開き、
/// 閉じるで閉じる。色スウォッチを2回続けて押してもモーダルは開かない
#[test]
fn color_edit_button_opens_modal_for_selected_slot() {
    let mut harness = app();
    let swatch = harness.get_by_label_contains("塗る色 3:").rect().center();
    harness.event(egui::Event::PointerMoved(swatch));
    frame(&mut harness);
    // ダブルクリックの間隔で押す・離すを2回
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
        harness.query_by_label("色をつくる").is_none(),
        "スウォッチの連打でモーダルが開いた"
    );

    harness.get_by_label("色をつくる(#3)…").click();
    frames(&mut harness, 2);
    assert!(
        harness.query_by_label("色をつくる").is_some(),
        "モーダルが開かない"
    );
    assert!(
        harness.query_by_label("#3").is_some(),
        "編集対象が #3 でない"
    );

    harness.get_by_label("閉じる").click();
    frames(&mut harness, 3);
    assert!(harness.query_by_label("色をつくる").is_none(), "閉じない");
}

/// 色スウォッチの右クリックメニュー「この色をつくる」で、その色の編集モーダルが開く
#[test]
fn swatch_context_menu_opens_color_modal() {
    let mut harness = app();
    harness.get_by_label_contains("塗る色 4:").click_secondary();
    frames(&mut harness, 2);
    harness.get_by_label("この色をつくる…").click();
    frames(&mut harness, 2);
    assert!(
        harness.query_by_label("色をつくる").is_some(),
        "モーダルが開かない"
    );
    assert!(
        harness.query_by_label("#4").is_some(),
        "編集対象が #4 でない"
    );
}

/// 「乾かす(固定)」で乾燥レイヤーに焼き込んでも色は残る。濡れ色(水量で最大 12% 暗く
/// 表示。display.wgsl)が抜けて少し明るくなるだけで、色相は変わらない
#[test]
fn dry_keeps_painted_color() {
    let mut harness = app();
    harness.get_by_label_contains("塗る色 2:").click();
    frames(&mut harness, 2);
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

/// 解析用キャプチャ: 描いてウィンドウを閉じようとすると、閉じる前に記録一式を書き出してから閉じる
/// (テストの描画では UI スクショが届かないので、待ちきれずに閉じる経路を通る。log.txt は main のログ初期化が作るので見ない)
#[test]
fn capture_writes_session_on_close() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("e2e-capture");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let capture_dir = dir.clone();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 780.0))
        .with_step_dt(1.0 / 60.0)
        .renderer(WgpuTestRenderer::from_render_state(render_state()))
        .build_eframe(move |cc| PaintApp::with_capture(cc, Some(capture_dir.clone())));
    frames(&mut harness, 3);
    harness.get_by_label_contains("塗る色 2:").click();
    frames(&mut harness, 2);
    mouse_drag(&mut harness, 0.3, 0.7, 0.5);
    frames(&mut harness, 10);

    harness
        .input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    frame(&mut harness);
    let closes = |h: &Harness<'_, PaintApp>| {
        h.output()
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::Close))
    };
    let cancelled = harness
        .output()
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose));
    assert!(cancelled, "閉じる要求を止めずにそのまま閉じた");
    let mut closed = false;
    for _ in 0..90 {
        frame(&mut harness);
        if closes(&harness) {
            closed = true;
            break;
        }
    }
    assert!(closed, "書き出しの後に閉じていない");

    for name in [
        "params_start.json",
        "params_end.json",
        "session.json",
        "strokes.json",
        "canvas.png",
        "work.kasane",
    ] {
        assert!(dir.join(name).is_file(), "{name} が書き出されていない");
    }
    let stored: kasaneiro::replay::StoredRecording =
        serde_json::from_slice(&std::fs::read(dir.join("strokes.json")).unwrap()).unwrap();
    assert_eq!(
        stored.recording.strokes.len(),
        1,
        "描いた1本が記録されていない"
    );
    assert!(
        stored.recording.strokes[0].load.is_some(),
        "筆の中身が記録されていない"
    );
}

/// ペン(筆圧付き Touch)でキャンバス上の点列(相対位置)を順に結んで引く。区間ごとに 10 フレームで進む
fn pen_path(harness: &mut Harness<'_, PaintApp>, path: &[(f32, f32)], force: f32) {
    let rect = canvas_rect(harness);
    let touch = |(u, v): (f32, f32), phase| egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase,
        pos: rect.lerp_inside(egui::vec2(u, v)),
        force: Some(force),
    };
    harness.event(touch(path[0], egui::TouchPhase::Start));
    frame(harness);
    for pair in path.windows(2) {
        for i in 1..=10 {
            let t = i as f32 / 10.0;
            let (a, b) = (pair[0], pair[1]);
            let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            harness.event(touch(p, egui::TouchPhase::Move));
            frame(harness);
        }
    }
    harness.event(touch(*path.last().unwrap(), egui::TouchPhase::End));
    frame(harness);
}

fn luma(c: [f32; 3]) -> f32 {
    (c[0] + c[1] + c[2]) / 3.0
}

/// 清書ペン: 別のストロークを重ねた所は濃くなり、1本の中で往復した所は濃くならない
#[test]
fn pen_overlapping_strokes_darken() {
    let mut harness = app();
    harness.get_by_label("清書(ペン)").click();
    frames(&mut harness, 2);
    pen_path(&mut harness, &[(0.2, 0.5), (0.8, 0.5)], 1.0);
    pen_path(&mut harness, &[(0.5, 0.3), (0.5, 0.7)], 1.0);
    pen_path(&mut harness, &[(0.2, 0.25), (0.8, 0.25), (0.2, 0.25)], 1.0);
    frames(&mut harness, 2);
    let c = canvas_color(
        &mut harness,
        "pen-overlap",
        &[(0.3, 0.5), (0.5, 0.5), (0.4, 0.25), (0.05, 0.05)],
    );
    let (single, cross, back_and_forth, paper) = (luma(c[0]), luma(c[1]), luma(c[2]), luma(c[3]));
    assert!(
        single < paper - 0.3,
        "線が描けていない: {single} vs 紙 {paper}"
    );
    assert!(
        cross < single - 0.05,
        "重ねても濃くならない: {cross} vs {single}"
    );
    assert!(
        (back_and_forth - single).abs() < 0.05,
        "1本の往復で濃さが変わった: {back_and_forth} vs {single}"
    );
}

/// 清書ペン: 弱い筆圧の細い線が点線に欠けず、薄く連続して描ける
#[test]
fn pen_light_pressure_line_is_continuous() {
    let mut harness = app();
    harness.get_by_label("清書(ペン)").click();
    frames(&mut harness, 2);
    pen_path(&mut harness, &[(0.2, 0.5), (0.8, 0.5)], 0.15);
    frames(&mut harness, 2);
    let at: Vec<(f32, f32)> = (0..20).map(|i| (0.3 + 0.02 * i as f32, 0.5)).collect();
    let mut probe = at.clone();
    probe.push((0.05, 0.05));
    let c = canvas_color(&mut harness, "pen-light", &probe);
    let paper = luma(*c.last().unwrap());
    let along: Vec<f32> = c[..at.len()].iter().map(|&x| luma(x)).collect();
    let (min, max) = along
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    assert!(max < paper - 0.03, "途切れた所がある: {along:?} 紙 {paper}");
    assert!(max - min < 0.1, "濃さがまだらに揺れる: {along:?}");
}

/// 清書ペン: 元に戻す → やり直しで、重なりの濃さも手ぶれ補正した線の形も描いたときと同じに戻る
#[test]
fn pen_undo_redo_restores_same_lines() {
    let mut harness = app();
    harness.get_by_label("清書(ペン)").click();
    frames(&mut harness, 2);
    pen_path(&mut harness, &[(0.2, 0.5), (0.8, 0.5)], 1.0);
    pen_path(
        &mut harness,
        &[(0.5, 0.3), (0.52, 0.45), (0.48, 0.55), (0.5, 0.7)],
        0.6,
    );
    frames(&mut harness, 2);
    let at: Vec<(f32, f32)> = (0..41)
        .flat_map(|i| {
            let t = 0.3 + 0.01 * i as f32;
            [(0.5, t), (0.49, t), (0.51, t), (t + 0.2, 0.5)]
        })
        .collect();
    let before = canvas_color(&mut harness, "pen-undo-before", &at);
    harness.get_by_label("元に戻す").click();
    frames(&mut harness, 2);
    harness.get_by_label("やり直し").click();
    frames(&mut harness, 2);
    let after = canvas_color(&mut harness, "pen-undo-after", &at);
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        let d = (0..3).map(|c| (b[c] - a[c]).abs()).fold(0.0, f32::max);
        assert!(d < 0.02, "{:?} で違う: {b:?} → {a:?}", at[i]);
    }
}

/// 清書ペンの消しゴムは描く太さとは別の太さを持つ: 細いペン線を既定の太い消しゴムで横切ると、
/// ペン幅より外まで消える。消しゴムを外すと細いペンに戻り、Undo → Redo でも消し跡の幅が変わらない
#[test]
fn pen_eraser_keeps_own_radius() {
    let mut harness = app();
    harness.get_by_label("清書(ペン)").click();
    frames(&mut harness, 2);
    // 太さはスライダーと数値欄の2ノードに同じ名前が付く
    let eraser_slider =
        |h: &Harness<'_, PaintApp>| h.query_all_by_label("消しゴムの太さ").count() > 0;
    assert!(!eraser_slider(&harness));
    pen_path(&mut harness, &[(0.2, 0.5), (0.8, 0.5)], 1.0);

    harness.get_by_label("消しゴム").click();
    frames(&mut harness, 2);
    assert!(
        eraser_slider(&harness),
        "消しゴム中に消しゴムの太さのスライダーが出ていない"
    );
    pen_path(&mut harness, &[(0.5, 0.3), (0.5, 0.7)], 1.0);
    harness.get_by_label("消しゴム").click();
    frames(&mut harness, 2);
    pen_path(&mut harness, &[(0.2, 0.7), (0.8, 0.7)], 1.0);
    frames(&mut harness, 2);

    // 512 テクセルのキャンバスで 7 テクセル = ペン(半径 4)の外・消しゴム(半径 10)の内、
    // 20 テクセル = 両方の外
    let near = 7.0 / 512.0;
    let far = 20.0 / 512.0;
    let at = [
        (0.5 + near, 0.5),
        (0.5 - near, 0.5),
        (0.5 + far, 0.5),
        (0.4, 0.7 + near),
        (0.1, 0.1),
    ];
    let before = canvas_color(&mut harness, "pen-eraser", &at);
    // 紙の地色(何も描いていない点)からの暗さ
    let ink = |c: [f32; 3]| luma(before[4]) - luma(c);
    assert!(
        ink(before[0]) < 0.03 && ink(before[1]) < 0.03,
        "消しゴムがペン幅より外まで消していない: {before:?}"
    );
    assert!(
        ink(before[2]) > 0.3,
        "消しゴムの外の線まで消えている: {before:?}"
    );
    assert!(
        ink(before[3]) < 0.03,
        "消しゴムを外したペンが太い: {before:?}"
    );

    for label in ["元に戻す", "元に戻す", "やり直し", "やり直し"] {
        harness.get_by_label(label).click();
        frames(&mut harness, 2);
    }
    let after = canvas_color(&mut harness, "pen-eraser-redo", &at);
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        let d = (0..3).map(|c| (b[c] - a[c]).abs()).fold(0.0, f32::max);
        assert!(d < 0.02, "{:?} で引き直しが違う: {b:?} → {a:?}", at[i]);
    }
}
