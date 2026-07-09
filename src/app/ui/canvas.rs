//! 中央のキャンバス描画(ポインタ入力の取り込み・記録再生の合流・シミュステップ数の決定)と
//! シェーダーエラーのオーバーレイ。app/mod.rs から分割(R4)。

use crate::app::PaintApp;
use crate::gpu::CanvasCallback;
use crate::input::PointerSource;
use paint_core::sim::Splat;
use eframe::egui;
use eframe::egui_wgpu;

impl PaintApp {
    pub(in crate::app) fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        // 正方形キャンバスを利用可能領域の中央に置く
        let available = ui.available_size();
        let side = available.min_elem().max(64.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(side, side),
            egui::Sense::drag(),
        );

        // M6: パン/ズーム/回転。ホイール=カーソル中心に拡大、Shift+ホイール=15°ずつ回転、
        // 中ボタン or スペース+左ドラッグ=パン。ポインタ状態はグローバル(ウィジェットの Sense に
        // 依らない)なので input から直接読む。回転は離散させたいので MouseWheel イベント(未平滑)で拾う
        // 名前入力欄などにフォーカス中はスペースを奪わない
        let typing = ui.ctx().egui_wants_keyboard_input();
        let (scroll_y, wheel_notch, shift, ctrl, alt, space, middle_down, primary_down, ptr_delta, hover) =
            ui.input(|i| {
                let wheel_notch: f32 = i
                    .events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                        _ => None,
                    })
                    .sum();
                (
                    i.smooth_scroll_delta.y,
                    wheel_notch,
                    i.modifiers.shift,
                    i.modifiers.ctrl,
                    i.modifiers.alt,
                    i.key_down(egui::Key::Space),
                    i.pointer.middle_down(),
                    i.pointer.primary_down(),
                    i.pointer.delta(),
                    i.pointer.hover_pos(),
                )
            });
        let over_canvas = hover.is_some_and(|p| rect.contains(p));
        if over_canvas && shift && wheel_notch != 0.0 {
            // Shift+ホイール: 1ノッチ 15°(上=反時計回り / 下=時計回り)
            self.rotate_view(wheel_notch.signum() * std::f32::consts::FRAC_PI_8 * 1.5);
        } else if over_canvas
            && scroll_y != 0.0
            && let Some(cursor) = hover
        {
            // ホイール量を対数スケールで拡大率に。上スクロール=拡大 / 下=縮小
            self.zoom_at(cursor, rect, (scroll_y * 0.0015).exp());
        }
        // パン: 中ボタンドラッグ、または スペース押下中の左ドラッグ(いずれもキャンバス上)。
        // スペース押下中は「パン意図」とみなし描画を抑止する(下の apply_pointer_events を飛ばす)。
        // 実際の移動は zoom>1 か回転ありのときだけ(全体表示・無回転では窓を動かす余地がない)
        let space_hold = space && !typing;
        let panning = (middle_down || (space_hold && primary_down)) && over_canvas;
        let can_move = self.view_zoom > 1.0 || self.view_angle != 0.0;
        if panning && can_move {
            let span = 1.0 / self.view_zoom;
            // 画面ドラッグと逆にキャンバス中心を動かす(回転込みで写す)
            let d = ptr_delta / rect.width().max(1.0);
            self.view_center -= self.view_rotate(d) * span;
            self.clamp_view();
        }
        // スペース押下中(左ドラッグでのパン意図)は描画を抑止する
        let panning = panning || (space_hold && over_canvas);

        // F18: Ctrl+Alt+左ドラッグでブラシ半径を変える(描く前でも最中でも太さを即調整できる)。
        // 横ドラッグ量(画面px)を現ツール半径(テクセル)へ加算する。画面px = テクセル×scale なので
        // px/scale 加算にすると「リングの縁がカーソルにぴたり追従」= 拡大率やキャンバスサイズに依らず
        // 一定の手応えになる。このジェスチャ中は描画を抑止し、リングと数値でサイズを見せる
        let resizing = ctrl && alt && primary_down && over_canvas;
        if resizing && ptr_delta.x != 0.0 {
            let scale = rect.width() / self.canvas_size as f32 * self.view_zoom;
            self.adjust_active_radius(ptr_delta.x / scale.max(1e-3));
        }

        // M1.5: ペン(egui Touch、筆圧付き)を優先し、接地中はマウスを無視する
        // (egui-winit は Touch からポインタもエミュレートするため、両方を処理すると
        // 二重ストロークになる)
        let pen_events = self.pen.poll(&response);
        let events = if self.pen.is_active() || !pen_events.is_empty() {
            pen_events
        } else {
            self.mouse.poll(&response)
        };

        let mut splats: Vec<Splat> = Vec::new();
        // M5e: スポイト待機中はクリックで色を拾うだけで、描画・記録はしない
        if self.palette_ui.eyedropper {
            let pressed = ui.input(|i| i.pointer.primary_pressed());
            if pressed
                && let Some(pos) = response.hover_pos()
                && rect.contains(pos)
            {
                self.pick_color(pos, rect);
                self.palette_ui.eyedropper = false;
            }
        } else if self.drawing_locked() {
            // 乾燥レイヤー選択中は描画不可(焼き込みは一方通行)。クリックされたら案内を出す
            let pressed = ui.input(|i| i.pointer.primary_pressed());
            if pressed && hover.is_some_and(|p| rect.contains(p)) {
                self.status_msg = Some(
                    "乾燥レイヤーは編集できません。右のレイヤーパネルで描くレイヤー(水彩・線画)を選択してください"
                        .to_owned(),
                );
            }
        } else if !panning && !resizing {
            // M6: パン中(中ボタンドラッグ)は描画イベントを流さない(ビュー操作専念)。
            // F18: Ctrl+Alt+ドラッグでの半径調整中も同様に描かない
            self.apply_pointer_events(&events, rect, &mut splats);
            // 筆の含み: 塗る筆を置いたまま動かないフレームでも色水が流れ出続ける
            let dt = ui.input(|i| i.stable_dt);
            self.feed_charge(dt, &mut splats);
        }

        // H5: 記録はフレーム基準(ストローク間の待ちも再現される)
        if let Some(recorder) = &mut self.replay_ui.recorder {
            recorder.tick();
        }

        // H5: 再生中は記録済みポインタ入力を同じテンポで流し込む(手描きと合流可)
        if let Some(player) = &mut self.replay_ui.player
            && !player.advance(&mut self.params, &mut splats)
        {
            self.stop_replay();
        }

        // H6: 一時停止中は 0 ステップ(1ステップボタンが押されていれば 1)
        let sim_steps = if self.paused {
            if std::mem::take(&mut self.step_once) { 1 } else { 0 }
        } else {
            self.steps_per_frame
        };

        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            CanvasCallback {
                params: self.params,
                splats,
                sim_steps,
                line_target: self.line_target(),
                view: self.view_uniform(),
            },
        ));
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            (1.0, ui.visuals().weak_text_color()),
            egui::StrokeKind::Outside,
        );

        // F18: ブラシサイズのカーソルリング。カーソル位置に実際の描画半径の輪郭を重ね、
        // 「今どのくらいの太さで描くか」を描く前・描いてる最中に見えるようにする。
        // 半径はテクセル単位(active_base_radius)なので画面 px へ = ×(rect幅/canvas_size)×zoom。
        // 明暗どちらの下地でも見えるよう黒→白の二重リングにする。パン中・スポイト待機・
        // 乾燥レイヤー(描画不可)では出さない
        let show_cursor = over_canvas
            && (resizing || (!panning && !self.palette_ui.eyedropper && !self.drawing_locked()));
        if show_cursor
            && let Some(pos) = hover
        {
            let base = self.active_base_radius();
            let scale = rect.width() / self.canvas_size as f32 * self.view_zoom;
            let r = (base * scale).max(1.5);
            let painter = ui.painter();
            painter.circle_stroke(pos, r + 0.5, (1.5, egui::Color32::from_black_alpha(140)));
            painter.circle_stroke(pos, r, (1.0, egui::Color32::from_white_alpha(210)));
            // F18: サイズ調整中は現在の半径(px)をリングの右に出す(黒縁つき白文字で下地に依らず読める)
            if resizing {
                let text = format!("{base:.0} px");
                let anchor = pos + egui::vec2(r + 8.0, 0.0);
                let font = egui::FontId::proportional(13.0);
                for (off, col) in [
                    (egui::vec2(1.0, 1.0), egui::Color32::from_black_alpha(180)),
                    (egui::Vec2::ZERO, egui::Color32::WHITE),
                ] {
                    painter.text(anchor + off, egui::Align2::LEFT_CENTER, &text, font.clone(), col);
                }
            }
        }

        // F15: 初回ガイド。まだ一度も描いていない間だけ、空キャンバス中央に淡いヒントを出す。
        // 最初の一筆(has_painted)で消える。開発機能・描画には一切影響しない
        if !self.has_painted {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "左で色を選び、ここにドラッグして描いてみましょう",
                egui::FontId::proportional(20.0),
                egui::Color32::from_rgba_unmultiplied(90, 90, 90, 150),
            );
        }
    }

    pub(in crate::app) fn error_overlay(&self, ui: &mut egui::Ui) {
        let Some(error) = &self.shader_error else {
            return;
        };
        egui::Panel::bottom("shader_error")
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(60, 16, 16))
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 140, 140),
                    "WGSL コンパイルエラー(直前の正常なシェーダーで継続中。保存し直すと再試行):",
                );
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(error).monospace().color(egui::Color32::WHITE),
                        )
                        .wrap(),
                    );
                });
            });
    }
}
