//! app の UI(R4)。UI 状態の構造体と、各パネル描画の `impl PaintApp`(per-file 分割)。
//!
//! [`super::PaintApp`] の肥大化していたフィールドを意味ごとの構造体にまとめる:
//! プリセット([`PresetUi`])・ストローク記録再生([`ReplayUi`])。各セクションの描画は
//! `impl PaintApp` メソッドとして以下のサブモジュールに分割してある(app/mod.rs 側は
//! 状態・ライフサイクルとディスパッチャ `tool_panel` だけを持つ):
//! - [`tools`] — 乾燥ボタン(常時表示)と、アクティブレイヤーごとのツールパネル
//!   (`dry_controls` / `active_tools_panel` が水彩・鉛筆・ペン・ハイライト・乾燥を出し分け)
//! - [`palette`] — 顔料パレット編集(`palette_panel`。M5。水彩レイヤー選択時のみ表示。
//!   色ライブラリ・パレット保存/読込を常時表示セクションとして持ち、「色をつくる」は
//!   色スウォッチのダブルクリックで開くモーダル `color_edit_modal`)
//! - [`layers`] — 右パネルのレイヤースタック(選択=ツール系統の切替・可視性・並べ替え・合成方式。
//!   `layer_stack_panel`)
//! - [`tuning`] — 乾燥・筆圧・味付けスライダー・診断・シミュ制御(`tuning_panel`)
//! - [`panels`] — 記録再生 / シェーダー状態 / 操作結果表示(`replay_panel` / `shader_status` / `status_bar`)
//! - [`file_menu`] — 上部「ファイル」メニューバー+モーダル(作品保存/開く・設定プリセット・新規キャンバス・全部消す。`menu_bar` / `file_modals`)
//! - [`canvas`] — キャンバス描画とエラーオーバーレイ(`canvas_ui` / `error_overlay`)
//!
//! プリセット(H3)とストローク(H5)で重複していた「名前入力+保存+一覧」パターンは
//! [`NamedStore`] に一本化した(save_controls / list_rows)。

mod canvas;
mod file_menu;
mod layers;
mod palette;
mod panels;
mod tools;
mod tuning;

use eframe::egui;
use paint_core::replay::{Player, Recorder, Recording};
use std::path::PathBuf;

/// 名前入力+保存+一覧の共通状態(プリセット H3 / ストローク H5 で重複していたパターン。R4)。
pub struct NamedStore {
    /// 保存名の入力欄
    pub name: String,
    /// 保存済み名の一覧(保存時と ↻ で更新するキャッシュ)
    pub list: Vec<String>,
}

impl NamedStore {
    pub fn new(list: Vec<String>) -> Self {
        Self {
            name: String::new(),
            list,
        }
    }

    /// 名前入力欄+保存ボタン+↻(一覧再読込)を横並びで描く。保存が押されたら `save(name)` を
    /// 実行し、表示用ステータス文字列を返す(押されなければ None)。保存成功時は一覧を更新する。
    pub fn save_controls(
        &mut self,
        ui: &mut egui::Ui,
        hint: &str,
        save: impl FnOnce(&str) -> Result<PathBuf, String>,
        relist: impl Fn() -> Vec<String>,
    ) -> Option<String> {
        let mut status = None;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .hint_text(hint)
                    .desired_width(140.0),
            );
            let name = self.name.trim().to_owned();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("保存"))
                .clicked()
            {
                status = Some(match save(&name) {
                    Ok(path) => {
                        self.list = relist();
                        format!("保存: {}", path.display())
                    }
                    Err(e) => e,
                });
            }
            if ui.button("↻").on_hover_text("一覧を再読込").clicked() {
                self.list = relist();
            }
        });
        status
    }

    /// 一覧を「[button_label] 名前」の行で並べる。押された名前を返す(なければ None)。
    pub fn list_rows(&self, ui: &mut egui::Ui, button_label: &str) -> Option<String> {
        let mut picked = None;
        for name in &self.list {
            ui.horizontal(|ui| {
                if ui.button(button_label).clicked() {
                    picked = Some(name.clone());
                }
                ui.label(name);
            });
        }
        picked
    }
}

/// プリセット(H3)の UI 状態。
pub struct PresetUi {
    pub store: NamedStore,
}

/// 作品保存(M7)の UI 状態。名前入力+保存済み一覧(`.kasane`)。
pub struct WorkUi {
    pub store: NamedStore,
}

/// パレット(M5)の UI 状態。ライブラリ保存/読込(M5d)+ スポイト待機(M5e)+
/// モーダル一覧のキャッシュ(M5f/g)。
pub struct PaletteUi {
    pub store: NamedStore,
    /// M5e: スポイト待機中。次のキャンバスクリックで色を拾い、選択スロットへ入れて解除する
    pub eyedropper: bool,
    /// M5f: 色ライブラリ一覧のキャッシュ(左パネルに常時表示するので起動時に読み、
    /// 保存後・↻ で更新)。色見本チップとホバーの ρ/ω/γ 表示のため Pigment 丸ごと持つ
    pub pigment_cache: Vec<(String, pigment::Pigment)>,
    /// M5g: パレット一覧のキャッシュ(4色見本チップ用。更新タイミングは pigment_cache と同じ)
    pub palette_cache: Vec<(String, pigment::Palette)>,
    /// 「色をつくる」モーダルの開閉。ツールバーの色スウォッチをダブルクリックで開く
    /// (編集対象は選択中スロット=1回目のクリックで切り替わったスロット)
    pub color_modal: bool,
}

/// ストローク記録・再生(H5)の UI 状態をまとめる(R4 で PaintApp の 5 フィールドを集約)。
pub struct ReplayUi {
    pub store: NamedStore,
    /// 記録中の状態(Some の間はポインタ入力を記録)
    pub recorder: Option<Recorder>,
    /// 記録停止後、保存/試し再生できる直近の記録
    pub pending_recording: Option<Recording>,
    /// 再生中の状態(Some の間は記録済み入力を毎フレーム流し込む)
    pub player: Option<Player>,
    /// 再生開始前に選択していた顔料スロット(再生終了時に戻す)。
    /// Player::advance が params.brush_channel を記録値で上書きするため
    pub saved_channel: Option<u32>,
}
