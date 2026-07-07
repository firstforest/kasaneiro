//! 乾燥・筆圧・味付けスライダー・診断表示・シミュ制御(H6)をまとめた調整セクション。
//! app/mod.rs から分割(R4)。

use crate::app::PaintApp;
use eframe::egui;

/// デバッグ表示モード(H4)。値は SimParams::display_mode / display.wgsl の分岐と対応。
const DISPLAY_MODES: [(u32, &str); 8] = [
    (0, "通常(顔料を表示)"),
    (1, "水量ヒートマップ"),
    (2, "速度場(色相=方向)"),
    (3, "湿りオーバーレイ(濡れ=青)"),
    (4, "浮遊顔料ヒートマップ"),
    (5, "沈着顔料ヒートマップ"),
    (6, "紙ハイト(白=山)"),
    (7, "アクティブタイル(計算中=緑)"),
];

impl PaintApp {
    /// 乾燥・筆圧・味付けスライダー・診断表示・シミュ制御(H6)をまとめた調整セクション
    pub(in crate::app) fn tuning_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("乾燥 (M2)");
        ui.add(
            egui::Slider::new(&mut self.params.dry_shift, 0.0..=1.5)
                .text("乾燥シフト(<1で乾くと薄く)"),
        );
        ui.add(
            egui::Slider::new(&mut self.params.rewet_water, 0.0..=2.0).text("再湿潤の水量"),
        );

        ui.separator();
        ui.heading("筆圧 (M1.5)");
        match self.pen.last_pressure() {
            Some(p) => {
                ui.colored_label(
                    egui::Color32::from_rgb(64, 160, 64),
                    format!("ペン接地中(筆圧 {p:.2})"),
                );
            }
            None => {
                ui.label("ペンでキャンバスに触れると筆圧が表示されます(マウスは 1.0 固定)");
            }
        }
        ui.add(egui::Slider::new(&mut self.params.pressure_radius, 0.0..=1.0).text("筆圧→半径の効き"));
        ui.add(egui::Slider::new(&mut self.params.pressure_water, 0.0..=1.0).text("筆圧→水量の効き"));
        ui.add(egui::Slider::new(&mut self.params.pressure_pigment, 0.0..=1.0).text("筆圧→顔料量の効き"));
        ui.add(
            egui::Slider::new(&mut self.params.pressure_gamma, 0.25..=4.0)
                .logarithmic(true)
                .text("応答カーブ γ(>1で軽いタッチが細く)"),
        );

        ui.separator();
        // 以降は物理シミュの内部係数と診断表示=制作者の味付け用ノブ。通常の描画では触らないため
        // 既定で畳んでおく(CLAUDE.md: パラメータ調整は制作者側の作業。普段はプリセットに封じ込める)。
        // 「乾燥の細部」もここへ移し、上の「乾燥 (M2)」は主ノブ(シフト・再湿潤)だけ残した
        egui::CollapsingHeader::new("調整パラメータ(味付け)")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("乾燥の細部 (M2)");
                ui.add(
                    egui::Slider::new(&mut self.params.dry_gran, 0.0..=1.0)
                        .text("焼き込み粒状感ゲート(凹部に濃く)"),
                );
                ui.add(
                    egui::Slider::new(&mut self.params.dry_edge, 0.0..=2.0)
                        .text("焼き込みエッジダークニング(縁バンド幅は M1d の値を共用)"),
                );

                ui.separator();
                ui.heading("水シミュレーション (M1a)");
                ui.add(egui::Slider::new(&mut self.params.dt, 0.05..=1.0).text("時間刻み dt"));
                ui.add(egui::Slider::new(&mut self.params.accel, 0.0..=4.0).text("移流強度(勾配→加速)"));
                ui.add(egui::Slider::new(&mut self.params.damping, 0.0..=0.5).text("速度減衰"));
                ui.add(egui::Slider::new(&mut self.params.xi, 0.0..=0.5).text("発散緩和 ξ"));
                ui.add(egui::Slider::new(&mut self.params.relax_iters, 1..=50).text("緩和反復回数"));
                ui.add(egui::Slider::new(&mut self.params.vel_max, 0.1..=2.0).text("速度上限 (CFL)"));
                ui.add(egui::Slider::new(&mut self.params.wet_expand, 0.0..=0.5).text("にじみ拡張(0=固定マスク)"));

                ui.separator();
                ui.heading("顔料 (M1b)");
                ui.add(egui::Slider::new(&mut self.params.pigment_diffuse, 0.0..=1.0).text("拡散率(にじみの速さ)"));
                ui.add(egui::Slider::new(&mut self.params.diffuse_iters, 0..=32).text("拡散反復回数(速いにじみはこちらで)"));
                ui.add(egui::Slider::new(&mut self.params.deposit_rate, 0.0..=0.5).text("吸着率(沈着の速さ)"));
                ui.add(egui::Slider::new(&mut self.params.lift_rate, 0.0..=0.5).text("脱着率(再浮遊の速さ)"));
                ui.add(
                    egui::Slider::new(&mut self.params.evap_rate, 0.0..=0.05)
                        .logarithmic(true)
                        .text("蒸発率"),
                );
                ui.add(
                    egui::Slider::new(&mut self.params.pigment_density, 0.5..=10.0)
                        .logarithmic(true)
                        .text("発色の濃さ(濃度→被覆率)"),
                );

                ui.separator();
                ui.heading("紙・エッジ (M1d)");
                ui.add(egui::Slider::new(&mut self.params.paper_amp, 0.0..=1.0).text("紙ハイト振幅(谷へ流す)"));
                ui.add(egui::Slider::new(&mut self.params.paper_gran, 0.0..=1.0).text("粒状化(凹部に沈着)"));
                ui.add(egui::Slider::new(&mut self.params.paper_wet, 0.0..=1.0).text("にじみ縁の紙目変調"));
                ui.add(
                    egui::Slider::new(&mut self.params.edge_eta, 0.0..=0.2)
                        .text("エッジダークニング η(0=無効)"),
                );
                ui.add(egui::Slider::new(&mut self.params.edge_radius, 1..=8).text("縁バンド幅(ぼかし半径)"));
            });

        // 診断表示(H4): デバッグ用ヒートマップ。通常描画は「通常」モード固定で十分なので畳む
        egui::CollapsingHeader::new("表示・診断 (H4)")
            .default_open(false)
            .show(ui, |ui| {
                let mode_label = |mode: u32| {
                    DISPLAY_MODES
                        .iter()
                        .find(|(v, _)| *v == mode)
                        .map_or("?", |(_, label)| *label)
                };
                egui::ComboBox::from_label("表示モード")
                    .selected_text(mode_label(self.params.display_mode))
                    .show_ui(ui, |ui| {
                        for (value, label) in DISPLAY_MODES {
                            ui.selectable_value(&mut self.params.display_mode, value, label);
                        }
                    });
                ui.add(
                    egui::Slider::new(&mut self.params.display_gain, 0.1..=10.0)
                        .logarithmic(true)
                        .text("表示ゲイン"),
                );

                ui.separator();
                ui.label("アクティブタイル (M6)");
                let mut active = self.params.active_tiles != 0;
                if ui
                    .checkbox(&mut active, "アクティブタイル最適化(濡れ面積に比例)")
                    .on_hover_text(
                        "濡れているタイル+ブラシ近傍だけシミュレーションを走らせる。\
                         オフにすると全面計算に戻る(A/B・不具合時の退避)。\
                         「アクティブタイル」表示モードで計算中の範囲を確認できる",
                    )
                    .changed()
                {
                    self.params.active_tiles = active as u32;
                }
            });

        ui.separator();
        ui.heading("制御 (H6)");
        ui.horizontal(|ui| {
            let pause_label = if self.paused { "▶ 再開" } else { "⏸ 一時停止" };
            if ui.button(pause_label).clicked() {
                self.paused = !self.paused;
            }
            if ui
                .add_enabled(self.paused, egui::Button::new("1ステップ"))
                .clicked()
            {
                self.step_once = true;
            }
        });
        ui.add(
            egui::Slider::new(&mut self.steps_per_frame, 1..=8)
                .text("速度倍率")
                .suffix(" ステップ/フレーム"),
        );
        ui.horizontal(|ui| {
            if ui.button("キャンバスをリセット").clicked() {
                self.clear_canvas();
            }
            if ui.button("PNG スナップショット").clicked() {
                self.save_snapshot();
            }
        });
        if ui
            .button("📷 UI スクショ (AI 用)")
            .on_hover_text(
                "画面全体(パネル込みの UI レイアウト)を screenshots/ui-latest.png へ上書き保存する。\
                 キャンバスだけの PNG スナップショットと違い、UI の見た目そのものを撮る。\
                 AI が同じパスから最新の UI を読めるので、UI 改善ループで使う",
            )
            .clicked()
        {
            self.request_ui_screenshot(ui.ctx());
        }

        // キャンバスサイズ(M8): 正方形 512/1024/2048。テクセル密度は据え置き=「広い紙」
        // (ブラシ・にじみの見た目スケールは変わらない)。変更は新規キャンバスの作り直しなので、
        // 描きかけは作品保存(M7)してから
        ui.horizontal(|ui| {
            ui.label("キャンバスサイズ");
            egui::ComboBox::from_id_salt("canvas_size")
                .selected_text(format!("{0}×{0}", self.pending_canvas_size))
                .show_ui(ui, |ui| {
                    for s in paint_core::sim::CANVAS_SIZES {
                        ui.selectable_value(&mut self.pending_canvas_size, s, format!("{s}×{s}"));
                    }
                });
            if ui
                .button("新規キャンバス")
                .on_hover_text(
                    "現在のキャンバスを破棄して選択サイズで作り直す(広い紙。テクセル密度は同じ)。\
                     保存していない絵は消えるので、残すなら先に作品保存 (M7) を",
                )
                .clicked()
            {
                let size = self.pending_canvas_size;
                self.recreate_canvas(size);
                self.status_msg = Some(format!("新規キャンバス: {size}×{size}"));
            }
        });
        if self.pending_canvas_size != self.canvas_size {
            ui.label(
                egui::RichText::new(format!(
                    "現在 {0}×{0}(「新規キャンバス」で切り替え)",
                    self.canvas_size
                ))
                .weak()
                .small(),
            );
        }
    }
}
