//! レイヤーパネル(M2)と合成方式(M3)。app/mod.rs から分割(R4)。

use crate::app::PaintApp;
use crate::gpu::{GpuCanvas, MAX_LAYERS};
use eframe::egui;

/// レイヤー合成方式(M3)。値は SimParams::compose_mode / display.wgsl の分岐と対応。
const COMPOSE_MODES: [(u32, &str, &str); 2] = [
    (0, "multiply", "M2 の乗算合成(重ねるほど暗く。散乱を無視した安価な近似)"),
    (
        1,
        "KM(R/T)",
        "Kubelka-Munk の光学合成(M3)。各層を白地/黒地に置いた発色から反射率・透過率を導き、下から光学混色する。薄い層ほど下が透ける「内側から光る」グレーズ",
    ),
];

impl PaintApp {
    /// M2: レイヤーパネル(可視性・並べ替え)。乾燥レイヤーは焼き込み後は編集不可で、
    /// multiply 合成では順序は見た目に効かない(KM 合成 M3 で効く)が配管は通しておく
    fn layer_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("レイヤー (M2)");
        let mut renderer = self.render_state.renderer.write();
        let Some(canvas) = renderer.callback_resources.get_mut::<GpuCanvas>() else {
            return;
        };
        ui.label(format!(
            "湿レイヤー(描画先)+ 乾燥 {}/{} 枚",
            canvas.layers.len(),
            MAX_LAYERS
        ));
        let count = canvas.layers.len();
        let mut changed = false;
        let mut swap: Option<(usize, usize)> = None;
        // 上から表示(Vec の末尾=最後に乾かしたもの=最上層)
        for k in (0..count).rev() {
            ui.horizontal(|ui| {
                let layer = &mut canvas.layers[k];
                if ui
                    .checkbox(&mut layer.visible, format!("乾燥レイヤー {}", layer.slot + 1))
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .add_enabled(k + 1 < count, egui::Button::new("⬆"))
                    .clicked()
                {
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
        }
        if changed {
            canvas.sync_layers(&self.render_state.queue);
        }
        drop(renderer);

        // 線画レイヤー(M4.5a/c): 位置固定・並べ替え対象外。表示切替のみ(色より上に合成)
        let mut show_pencil = self.params.show_pencil != 0;
        let mut show_pen = self.params.show_pen != 0;
        let mut show_highlight = self.params.show_highlight != 0;
        if ui.checkbox(&mut show_pencil, "下書き(鉛筆)").changed() {
            self.params.show_pencil = show_pencil as u32;
        }
        if ui.checkbox(&mut show_pen, "清書(ペン)").changed() {
            self.params.show_pen = show_pen as u32;
        }
        if ui.checkbox(&mut show_highlight, "ハイライト(白)").changed() {
            self.params.show_highlight = show_highlight as u32;
        }
    }

    /// レイヤー(M2): 乾燥レイヤーの可視性・並べ替え + 合成方式(M3)
    pub(in crate::app) fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        self.layer_panel(ui);
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
