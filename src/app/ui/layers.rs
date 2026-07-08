//! 右パネルのレイヤースタック(M2/M4.5)と合成方式(M3)。
//!
//! 上から合成順(手前 → 奥)に、ハイライト → 清書ペン → 下書き鉛筆 → 水彩(湿)→
//! 乾燥レイヤー → 紙、を1列に並べる。各行 = 表示チェック + 選択ラベル。
//! **選択中のレイヤーがそのままツール系統の切替**([`PaintApp::select_layer`])で、
//! 左パネルにはそのレイヤーのツールだけが出る(active_tools_panel)。
//! 乾燥レイヤーは焼き込み済みで編集不可(選択はできるが描画はブロックし、並べ替えのみ)。

use crate::app::{ActiveLayer, PaintApp};
use crate::gpu::{GpuCanvas, MAX_LAYERS};
use eframe::egui;

/// レイヤー合成方式(M3)。値は SimParams::compose_mode / display.wgsl の分岐と対応。
const COMPOSE_MODES: [(u32, &str, &str); 2] = [
    (0, "乗算(重ねて暗く)", "multiply(M2)の乗算合成(重ねるほど暗く。散乱を無視した安価な近似)"),
    (
        1,
        "グレーズ(内側から光る)",
        "Kubelka-Munk の光学合成(M3)。各層を白地/黒地に置いた発色から反射率・透過率を導き、下から光学混色する。薄い層ほど下が透ける「内側から光る」グレーズ",
    ),
];

/// レイヤー1行: 表示チェックボックス + 選択ラベル。`visible = None` は常時表示のレイヤー
/// (水彩の描画先)で、チェックを無効表示にする。戻り値 = (ラベルがクリックされた, 表示が変わった)
fn layer_row(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    hint: &str,
    visible: Option<&mut bool>,
) -> (bool, bool) {
    let mut clicked = false;
    let mut vis_changed = false;
    ui.horizontal(|ui| {
        match visible {
            Some(v) => {
                vis_changed = ui.checkbox(v, "").on_hover_text("表示/非表示").changed();
            }
            None => {
                let mut always = true;
                ui.add_enabled(false, egui::Checkbox::new(&mut always, ""))
                    .on_disabled_hover_text("描画先レイヤーは常に表示");
            }
        }
        clicked = ui.selectable_label(selected, label).on_hover_text(hint).clicked();
    });
    (clicked, vis_changed)
}

impl PaintApp {
    /// 右パネル: レイヤースタック。選択中レイヤーをハイライトし、選択がツール系統を切り替える。
    /// 乾燥レイヤーの可視性・並べ替え(M2)と合成方式(M3)もここに集約する
    pub(in crate::app) fn layer_stack_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.heading("レイヤー");
        // レイヤー構造の初回ガイド。右パネル幅で読みやすいよう短い2行に手動で分け、
        // .small() は使わず通常サイズの .weak() で表示する(小さすぎると読めないため)
        ui.label(egui::RichText::new("上=手前。選んだレイヤーのツールが左パネルに出ます").weak());
        ui.label(egui::RichText::new("おすすめ: 下書き→彩色の順").weak());
        ui.separator();

        // 選択は行の描画後にまとめて反映する(乾燥レイヤーのループが renderer を借りるため)
        let mut clicked_layer: Option<ActiveLayer> = None;

        // --- 線画レイヤー(位置固定・並べ替え対象外。色より上に合成)---
        let mut vis = self.params.show_highlight != 0;
        let (clicked, changed) = layer_row(
            ui,
            self.active_layer == ActiveLayer::Highlight,
            "ハイライト(白)",
            "不透明な白ブラシの最上段レイヤー(流体なし。M4.5c)",
            Some(&mut vis),
        );
        if changed {
            self.params.show_highlight = vis as u32;
        }
        if clicked {
            clicked_layer = Some(ActiveLayer::Highlight);
        }

        let mut vis = self.params.show_pen != 0;
        let (clicked, changed) = layer_row(
            ui,
            self.active_layer == ActiveLayer::Pen,
            "清書(ペン)",
            "清書ペンの線画。水の境界にもなる(M4.5a/b)",
            Some(&mut vis),
        );
        if changed {
            self.params.show_pen = vis as u32;
        }
        if clicked {
            clicked_layer = Some(ActiveLayer::Pen);
        }

        let mut vis = self.params.show_pencil != 0;
        let (clicked, changed) = layer_row(
            ui,
            self.active_layer == ActiveLayer::Pencil,
            "下書き(鉛筆)",
            "下書き鉛筆の線画(M4.5a)",
            Some(&mut vis),
        );
        if changed {
            self.params.show_pencil = vis as u32;
        }
        if clicked {
            clicked_layer = Some(ActiveLayer::Pencil);
        }

        // --- 水彩の湿レイヤー(流体シミュの描画先。常時表示)---
        let (clicked, _) = layer_row(
            ui,
            self.active_layer == ActiveLayer::Wet,
            "水彩(乾く前)",
            "水彩の描画先。「乾かす(固定)」で下の乾いた層へ固定される(M2 焼き込み)",
            None,
        );
        if clicked {
            clicked_layer = Some(ActiveLayer::Wet);
        }

        ui.separator();

        // --- 乾燥レイヤー(M2): 可視性・並べ替え・選択(編集は不可)---
        let mut sanitize_to_wet = false;
        {
            let mut renderer = self.render_state.renderer.write();
            let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() else {
                return;
            };
            let count = canvas.layers.len();
            // リセット・作品読込で選択中の乾燥レイヤーが消えていたら水彩へ戻す
            if let ActiveLayer::Dried(i) = self.active_layer
                && i >= count
            {
                sanitize_to_wet = true;
            }
            ui.label(
                egui::RichText::new(format!("乾いた層 {count}/{MAX_LAYERS} 枚(固定済み・編集不可)"))
                    .weak()
                    .small(),
            );
            let active = self.active_layer;
            let mut changed = false;
            let mut swap: Option<(usize, usize)> = None;
            // 上から表示(Vec の末尾=最後に乾かしたもの=最上層)
            for k in (0..count).rev() {
                let layer = &mut canvas.layers[k];
                let label = format!("乾いた層 {}", layer.slot + 1);
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut layer.visible, "").on_hover_text("表示/非表示").changed() {
                        changed = true;
                    }
                    if ui
                        .selectable_label(active == ActiveLayer::Dried(k), label)
                        .on_hover_text("乾いて固定済みのため編集不可。再編集は「全体を濡らす」で再湿潤してから")
                        .clicked()
                    {
                        clicked_layer = Some(ActiveLayer::Dried(k));
                    }
                    if ui.add_enabled(k + 1 < count, egui::Button::new("⬆")).clicked() {
                        swap = Some((k, k + 1));
                    }
                    if ui.add_enabled(k > 0, egui::Button::new("⬇")).clicked() {
                        swap = Some((k, k - 1));
                    }
                });
            }
            if let Some((a, b)) = swap {
                canvas.layers.swap(a, b);
                changed = true;
                // 選択中の乾燥レイヤーが並べ替えで動いたら選択を追従させる
                if self.active_layer == ActiveLayer::Dried(a) {
                    self.active_layer = ActiveLayer::Dried(b);
                } else if self.active_layer == ActiveLayer::Dried(b) {
                    self.active_layer = ActiveLayer::Dried(a);
                }
            }
            if changed {
                canvas.sync_layers(&self.render_state.queue);
            }
        }
        if sanitize_to_wet {
            self.select_layer(ActiveLayer::Wet);
        }
        if let Some(layer) = clicked_layer {
            self.select_layer(layer);
        }

        // --- 紙(最下層。操作対象外)---
        ui.horizontal(|ui| {
            let mut always = true;
            ui.add_enabled(false, egui::Checkbox::new(&mut always, ""));
            ui.label(egui::RichText::new("紙(最下層)").weak());
        });

        ui.separator();
        // レイヤー合成方式(M3): multiply(M2)⇔ KM の R/T 合成を切替。H5 再生で A/B 比較
        ui.horizontal(|ui| {
            ui.label("合成:");
            for (value, label, hover) in COMPOSE_MODES {
                ui.selectable_value(&mut self.params.compose_mode, value, label)
                    .on_hover_text(hover);
            }
        });
    }
}
