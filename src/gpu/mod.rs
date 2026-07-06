//! wgpu リソース管理: シミュレーションテクスチャ(水 / 浮遊顔料 / 沈着顔料、
//! いずれも rgba32float の ping-pong ペア。加えて紙ハイト r32float 1枚=静的)、
//! compute パス群(splat / 速度更新 / 発散緩和 / FlowOutward / 移流 / 拡散 /
//! 吸着・脱着+蒸発)、表示用パイプライン、PNG スナップショット用の読み戻し(H6)、
//! WGSL の実行時ロードと再ビルド(H1)。
//!
//! 乾燥レイヤー(M2): rgba32float の texture array(スライス = レイヤー、rgba = 4顔料濃度)。
//! 「乾かす」= bake パスで湿レイヤーの顔料を新スライスへ焼き込み、湿レイヤーを全ゼロに戻す。
//! RGB でなく顔料濃度のまま持つので、表示(multiply)は毎フレーム mixbox latent で発色でき、
//! KM 合成(M3)への置換もレイヤーデータを作り直さずに済む。合成順・可視性は LayerUniform。
//!
//! ping-pong は 3 テクスチャまとめて単一の `current` で管理する。各 compute パスは
//! 3 枚の src を読み、3 枚の dst を必ず全テクセル書いて(変更しない分は素通し)反転する。
//! パスごとに別の index を持つより素通しコストを払う方が単純で、512² では十分軽い。
//!
//! GpuCanvas は egui-wgpu の callback_resources に置かれ、次の2経路から触られる:
//! - フレームごとの描画は [`CanvasCallback`](callback)(prepare で compute、paint で表示)
//! - ホットリロード時の再ビルドは app.rs から rebuild_pipelines()
//!
//! ファイル構成(このモジュールはリソース定義と型・実行時メソッドを持つ。長い処理は分離):
//! - [`init`] — `GpuCanvas::new`(テクスチャ・バッファ・bind group の生成)
//! - [`callback`] — `CanvasCallback`(フレーム描画。パス実行順の正典)
//! - [`snapshot`] — `GpuCanvas::snapshot`(PNG 読み戻し。H6)
//! - [`shader_error`] — WGSL エラーの行番号補正(R3 QoL)

pub mod hot_reload;

mod callback;
mod init;
mod shader_error;
mod snapshot;

pub use callback::CanvasCallback;

use paint_core::sim::{CANVAS_SIZE, MAX_SPLATS, SimParams, Splat, SplatHeader};
use shader_error::remap_shader_error_lines;
use eframe::egui_wgpu::wgpu;
use std::collections::HashMap;
use std::path::PathBuf;

/// シミュレーションテクスチャのフォーマット。レイアウトは common.wgsl のコメントと対応。
const SIM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// ping-pong するテクスチャの種類数(水 / 浮遊顔料 / 沈着顔料)。
/// 紙ハイト(M1d)は静的なので含まない(paper_texture として別持ち)
const TEX_KINDS: usize = 3;

/// 乾燥レイヤー(M2)の上限枚数。texture array のスライス数として固定確保する
/// (512² rgba32float × 8 = 32MB。display.wgsl の visible_mask が u32 なので 32 が理論上限)
pub const MAX_LAYERS: usize = 8;

/// 紙ハイト生成のシード。「紙を作り直す」UI を足すならここをフィールド化する
const PAPER_SEED: u32 = 0x5EED;

/// latents uniform が持つパレット枠数(M5c)。乾燥レイヤー MAX_LAYERS 個 + 現行(live)1 個。
/// dried スロット s(0..MAX_LAYERS)は乾かした瞬間に現行パレットを焼き込んで固定し、
/// 現行(live)パレット枠 = 末尾(`LIVE_PALETTE`)は編集のたびに書き替える
pub const PALETTE_SLOTS: usize = MAX_LAYERS + 1;

/// 現行(live)パレットの枠番号(dried スロットの後ろ)。湿レイヤーの発色はここを使う
const LIVE_PALETTE: usize = MAX_LAYERS;

/// latents uniform の総 vec4 数 = グローバル光学 + パレット数 × 顔料ブロック。
/// display.wgsl の `array<vec4f, LATENT_TOTAL>` と一致させること(78 = 6 + 9×8)
pub const LATENT_TOTAL: usize = pigment::GLOBAL_LATENTS + PALETTE_SLOTS * pigment::PIGMENT_LATENTS;

/// 発散緩和の反復回数の上限(スライダー範囲より広めの安全弁)
const MAX_RELAX_ITERS: u32 = 64;

/// 顔料拡散の反復回数の上限(スライダー範囲より広めの安全弁)
const MAX_DIFFUSE_ITERS: u32 = 32;

/// アクティブタイル(M6): 1タイルの1辺テクセル数。common.wgsl の TILE_SIZE と一致させること
const TILE_SIZE: u32 = 16;
/// キャンバス1辺あたりのタイル数(= CANVAS_SIZE / TILE_SIZE)。common.wgsl の TILES_PER_SIDE と一致
const TILES_PER_SIDE: u32 = CANVAS_SIZE / TILE_SIZE;
/// アクティブタイルのフラグ数(タイル総数 = active/raw バッファの要素数)
const NUM_TILES: u32 = TILES_PER_SIDE * TILES_PER_SIDE;

/// compute シェーダーのバインドグループレイアウト種別(R3)。
/// ほとんどは共通レイアウト、bake だけ専用(乾燥レイヤースライス binding 9)。
#[derive(Clone, Copy)]
enum ComputeLayout {
    Common,
    Bake,
    /// ラスタ線画(M4.5a): 対象の線画テクスチャ(read_write)+ params + splats + 紙ハイト
    Raster,
    /// アクティブタイル走査(M6、tilescan): 水/浮遊/沈着(read)+ params + splats + raw_active(write)
    TileScan,
    /// アクティブタイル膨張(M6、tiledilate): raw_active(read)→ active(write)
    TileDilate,
}

/// ラスタ線画(M4.5a/c)の描画先テクスチャ。Tool::Raster の種別と対応。
/// 流体を通らず linesplat.wgsl が対応する read_write 線画テクスチャへ直接書く
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LineTarget {
    Pencil,
    Pen,
    /// 白ハイライト(M4.5c)。流体は通らず、合成の最上段に白を重ねる
    Highlight,
}

impl LineTarget {
    fn index(self) -> usize {
        match self {
            LineTarget::Pencil => 0,
            LineTarget::Pen => 1,
            LineTarget::Highlight => 2,
        }
    }
}

/// compute パイプラインの定義表(R3)。**シェーダー追加 = ここに1行**。
/// キーは WGSL ファイル名で、`Pipelines::compute()` の名前引きと prepare() の
/// パス実行順(ハードコード)が同じ名前を参照する。ここが持つのは「どのファイルを
/// どのレイアウトでビルドするか」だけで、実行順は心臓部なので prepare() 側に残す。
const COMPUTE_SHADERS: &[(&str, ComputeLayout)] = &[
    ("splat.wgsl", ComputeLayout::Common),
    ("velocity.wgsl", ComputeLayout::Common),
    ("relax.wgsl", ComputeLayout::Common),
    ("flowout.wgsl", ComputeLayout::Common),
    ("advect.wgsl", ComputeLayout::Common),
    ("diffuse.wgsl", ComputeLayout::Common),
    ("transfer.wgsl", ComputeLayout::Common),
    ("bake.wgsl", ComputeLayout::Bake),
    ("fastdry.wgsl", ComputeLayout::Common),
    ("rewet.wgsl", ComputeLayout::Common),
    ("linesplat.wgsl", ComputeLayout::Raster),
    // アクティブタイル(M6): シミュ本体より前にタイル有効フラグを作る2パス
    ("tilescan.wgsl", ComputeLayout::TileScan),
    ("tiledilate.wgsl", ComputeLayout::TileDilate),
];

struct Pipelines {
    /// COMPUTE_SHADERS を WGSL ファイル名で引く compute パイプライン群
    compute: HashMap<&'static str, wgpu::ComputePipeline>,
    display: wgpu::RenderPipeline,
    /// display と同じシェーダーを PNG スナップショット用フォーマットで焼くパイプライン(H6)
    snapshot: wgpu::RenderPipeline,
}

impl Pipelines {
    /// WGSL ファイル名で compute パイプラインを引く。ビルド成功時は COMPUTE_SHADERS が
    /// 全て揃っているので、未登録名(タイプミス)は開発時に落として気付けるよう panic する
    fn compute(&self, name: &str) -> &wgpu::ComputePipeline {
        self.compute
            .get(name)
            .unwrap_or_else(|| panic!("compute パイプライン {name} が未登録です(COMPUTE_SHADERS を確認)"))
    }
}

/// 乾燥レイヤー(M2)1枚分のメタ情報。実体は dried_texture のスライス `slot`。
/// Vec の並び = 重ね順(先頭が最下層)。multiply 合成では順序は見た目に効かないが、
/// KM 合成(M3)で効くため UI の並べ替えをそのまま uniform へ流す
#[derive(Clone, Copy)]
pub struct DriedLayer {
    /// dried_texture のスライス番号(焼き込み順に採番。全消去以外で解放しない)
    pub slot: u32,
    pub visible: bool,
}

/// display.wgsl の ViewUniform と同レイアウト(32 バイト)。パン/ズーム/回転(M6)。
/// 画面 uv → キャンバス uv の写像 canvas_uv = center + R(θ)·(uv − 0.5)·span を display へ渡す。
/// center = 画面中心に来るキャンバス uv、span = 1/zoom、R(θ) = 表示中心まわりの回転。
/// SimParams とは分けて display 専用 uniform にしている(プリセット H3・記録 H5 を汚さない)。
/// 回転で窓の隅がキャンバス外に出るぶんは display が背景色で塗る。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewUniform {
    /// 画面中心(uv=0.5,0.5)に来るキャンバス uv(0..1)
    pub center: [f32; 2],
    /// 表示窓の幅(キャンバス uv 単位)= 1/zoom。1.0 で全体、小さいほど拡大
    pub span: f32,
    /// 表示回転 θ の cos/sin(画面中心まわり。app 側で保持する角度から算出)
    pub cos_t: f32,
    pub sin_t: f32,
    pub _pad: [f32; 3],
}

impl Default for ViewUniform {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            span: 1.0,
            cos_t: 1.0,
            sin_t: 0.0,
            _pad: [0.0; 3],
        }
    }
}

/// display.wgsl の LayerUniform と同レイアウト(48 バイト)
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    count: u32,
    /// bit k = 下から k 番目のレイヤーの可視性
    visible_mask: u32,
    _pad: [u32; 2],
    /// order[k] = 下から k 番目のレイヤーのスロット番号
    order: [u32; MAX_LAYERS],
}

pub struct GpuCanvas {
    shader_dir: PathBuf,
    target_format: wgpu::TextureFormat,
    /// [水, 浮遊顔料, 沈着顔料] × ping-pong 2枚
    textures: [[wgpu::Texture; 2]; TEX_KINDS],
    /// textures と同順のビュー(bake の bind group をボタン押下時に組むため保持)
    sim_views: [[wgpu::TextureView; 2]; TEX_KINDS],
    /// 湿レイヤーの 1 段 undo(M6): ストローク開始時に current の [水, 浮遊, 沈着] を退避する。
    /// GPU 間コピー専用(bind しない)。restore_wet で current へ書き戻す
    wet_backup: [wgpu::Texture; TEX_KINDS],
    paper_view: wgpu::TextureView,
    compute_bind_groups: [wgpu::BindGroup; 2],
    display_bind_groups: [wgpu::BindGroup; 2],
    params_buffer: wgpu::Buffer,
    splat_buffer: wgpu::Buffer,
    /// 顔料+光学 latent(M1c/M5c)。display の binding 4。set_palette / bake_dry が書き替える。
    /// レイアウト = グローバル光学 6 vec4 + パレット PALETTE_SLOTS 個 × PIGMENT_LATENTS vec4
    latents_buffer: wgpu::Buffer,
    /// 顔料個性 ρ/ω/γ(M3)。compute の binding 9。M5 で set_palette がランタイム書き替え
    physics_buffer: wgpu::Buffer,
    /// パン/ズーム(M6)。display の binding 11。フレームごとに CanvasCallback が書き替える
    view_buffer: wgpu::Buffer,
    /// 現行(live)パレットの顔料 latent ブロック。乾かすとき dried スロットへ焼き込む(M5c)
    live_pigment_latents: [[f32; 4]; pigment::PIGMENT_LATENTS],
    compute_layout: wgpu::PipelineLayout,
    display_layout: wgpu::PipelineLayout,
    /// 乾燥レイヤー(M2): スライス = レイヤースロット、rgba = 4顔料濃度
    dried_slice_views: Vec<wgpu::TextureView>,
    layers_buffer: wgpu::Buffer,
    bake_bgl: wgpu::BindGroupLayout,
    bake_layout: wgpu::PipelineLayout,
    /// 線画(M4.5a/c): [鉛筆, ペン, ハイライト] の r32float テクスチャ(read_write。ping-pong しない)。
    /// clear() / clear_line() でゼロに戻すため実体を保持する
    line_textures: [wgpu::Texture; 3],
    /// linesplat.wgsl 用の描画先別 bind group([鉛筆, ペン, ハイライト]。LineTarget::index の順)
    raster_bind_groups: [wgpu::BindGroup; 3],
    raster_layout: wgpu::PipelineLayout,
    /// アクティブタイル(M6): タイル走査/膨張の bind group。実体のバッファ
    /// (raw_active / active、array<u32, NUM_TILES>)は bind group が保持する。
    /// tilescan の bind group は current テクスチャを読むので ping-pong parity 別に2つ
    tilescan_bind_groups: [wgpu::BindGroup; 2],
    /// tiledilate の bind group(raw→active。バッファ固定なので1つ)
    tiledilate_bind_group: wgpu::BindGroup,
    tilescan_layout: wgpu::PipelineLayout,
    tiledilate_layout: wgpu::PipelineLayout,
    /// 乾燥レイヤーの重ね順とメタ情報(先頭が最下層)。UI(app.rs)から直接編集し、
    /// 変更後に sync_layers() で uniform へ反映する
    pub layers: Vec<DriedLayer>,
    /// シェーダーが一度も通っていない/壊れている間は None(描画をスキップして継続)
    pipelines: Option<Pipelines>,
    /// 表示中のテクスチャ番号。各 compute パスは current を読み 1-current へ書いてから反転する
    current: usize,
    /// PNG スナップショット(H6)用: オフスクリーンターゲットと読み戻しバッファ
    snapshot_format: wgpu::TextureFormat,
    snapshot_texture: wgpu::Texture,
    snapshot_view: wgpu::TextureView,
    snapshot_buffer: wgpu::Buffer,
}

impl GpuCanvas {
    /// キャンバスをリセット(水・速度・濡れマスク・顔料をゼロに = 乾いた白い紙)。
    /// 乾燥レイヤー(M2)も全て破棄する(スロットは焼き込み時に上書きされるため
    /// テクスチャ自体はクリア不要。count=0 で表示から消える)
    pub fn clear(&mut self, queue: &wgpu::Queue) {
        // rgba32float 1 テクセル = 16 バイト。全ゼロ = 水なし・顔料なし・全面乾燥
        let zeros = vec![0u8; (CANVAS_SIZE * CANVAS_SIZE * 16) as usize];
        for texture in self.textures.iter().flatten() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &zeros,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(CANVAS_SIZE * 16),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: CANVAS_SIZE,
                    height: CANVAS_SIZE,
                    depth_or_array_layers: 1,
                },
            );
        }
        // 線画(M4.5a): r32float = 4 バイト/テクセル。全ゼロ = 線なし
        for texture in &self.line_textures {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &zeros[..(CANVAS_SIZE * CANVAS_SIZE * 4) as usize],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(CANVAS_SIZE * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: CANVAS_SIZE,
                    height: CANVAS_SIZE,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.layers.clear();
        self.sync_layers(queue);
    }

    /// 湿レイヤーの 1 段 undo(M6): いま表示中(current)の [水, 浮遊, 沈着] を退避テクスチャへ
    /// GPU 間コピーする。水彩ストローク開始(Down)時に呼ぶ。ping-pong の反転より前の
    /// 「ストローク直前の状態」を捉える(この後のフレームで splat + シミュが current を書き替える)
    pub fn backup_wet(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.copy_wet(device, queue, "wet_backup", true);
    }

    /// 湿レイヤーの 1 段 undo(M6): 退避テクスチャを current へ書き戻す。Ctrl+Z(水彩)で呼ぶ。
    /// current に書くので ping-pong の parity に依らず、次フレームの表示・シミュがここから続く
    pub fn restore_wet(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.copy_wet(device, queue, "wet_restore", false);
    }

    /// backup(current→退避)/ restore(退避→current)の共通実装。`backup=true` で退避方向。
    fn copy_wet(&self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, backup: bool) {
        let extent = wgpu::Extent3d {
            width: CANVAS_SIZE,
            height: CANVAS_SIZE,
            depth_or_array_layers: 1,
        };
        fn at(texture: &wgpu::Texture) -> wgpu::TexelCopyTextureInfo<'_> {
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(label),
        });
        for kind in 0..TEX_KINDS {
            let live = &self.textures[kind][self.current];
            let saved = &self.wet_backup[kind];
            let (src, dst) = if backup { (live, saved) } else { (saved, live) };
            encoder.copy_texture_to_texture(at(src), at(dst), extent);
        }
        queue.submit([encoder.finish()]);
    }

    /// 線画(M4.5a)の描画先 bind group を返す。CanvasCallback の prepare() が
    /// ラスタツールのとき linesplat.wgsl に渡す
    pub(crate) fn raster_bind_group(&self, target: LineTarget) -> &wgpu::BindGroup {
        &self.raster_bind_groups[target.index()]
    }

    /// 線画テクスチャ1枚をゼロに戻す(M4.5d の Undo で対象を再ラスタライズする前に呼ぶ)。
    /// r32float = 4 バイト/テクセル
    pub fn clear_line(&self, queue: &wgpu::Queue, target: LineTarget) {
        let zeros = vec![0u8; (CANVAS_SIZE * CANVAS_SIZE * 4) as usize];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.line_textures[target.index()],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &zeros,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(CANVAS_SIZE * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: CANVAS_SIZE,
                height: CANVAS_SIZE,
                depth_or_array_layers: 1,
            },
        );
    }

    /// 1ストロークを対象の線画テクスチャへ引き直す(M4.5d の Undo/Redo 再ラスタライズ)。
    /// 保存済みの実効パラメータ(params)で linesplat を回すので、現在のスライダー値に依らず
    /// 過去の線の形が保たれる。splat 数が MAX_SPLATS を超える長い線は分割して dispatch する。
    /// フレーム外の即時実行(params_buffer は次フレームの prepare() が上書きする)
    pub fn rasterize_line(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: LineTarget,
        params: &SimParams,
        splats: &[Splat],
    ) -> Result<(), String> {
        let pipelines = self.pipelines.as_ref().ok_or("シェーダーが未ビルドです")?;
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(params));
        let workgroups = CANVAS_SIZE.div_ceil(8);
        for chunk in splats.chunks(MAX_SPLATS) {
            let header = SplatHeader {
                count: chunk.len() as u32,
                _pad: [0; 3],
            };
            let mut bytes = Vec::with_capacity(
                std::mem::size_of::<SplatHeader>() + std::mem::size_of_val(chunk),
            );
            bytes.extend_from_slice(bytemuck::bytes_of(&header));
            bytes.extend_from_slice(bytemuck::cast_slice(chunk));
            queue.write_buffer(&self.splat_buffer, 0, &bytes);

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rasterize_line_encoder"),
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("rasterize_line_pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipelines.compute("linesplat.wgsl"));
                pass.set_bind_group(0, &self.raster_bind_groups[target.index()], &[]);
                pass.dispatch_workgroups(workgroups, workgroups, 1);
            }
            queue.submit([encoder.finish()]);
        }
        Ok(())
    }

    /// レイヤーの並び・可視性(self.layers)を display 用 uniform へ反映する(M2)。
    /// app.rs がレイヤーパネルで layers を編集したあとに呼ぶ
    pub fn sync_layers(&self, queue: &wgpu::Queue) {
        let mut uniform = LayerUniform {
            count: self.layers.len().min(MAX_LAYERS) as u32,
            visible_mask: 0,
            _pad: [0; 2],
            order: [0; MAX_LAYERS],
        };
        for (k, layer) in self.layers.iter().take(MAX_LAYERS).enumerate() {
            uniform.order[k] = layer.slot;
            if layer.visible {
                uniform.visible_mask |= 1 << k;
            }
        }
        queue.write_buffer(&self.layers_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    /// 現行(live)パレットを GPU へ反映する(M5b)。編集のたびに app から呼ぶ。
    /// - 顔料個性 ρ/ω/γ(physics)は全レイヤー共通=湿シミュ専用なので丸ごと書き替え
    /// - 顔料 latent(色)は live パレット枠だけ書き替える。乾燥済みレイヤーは記録済み
    ///   スロットを参照するため色が遡って変わらない(M5c)
    ///
    /// バッファは COPY_DST 済みなのでパイプライン再構築は不要
    pub fn set_palette(&mut self, queue: &wgpu::Queue, palette: &pigment::Palette) {
        let physics = palette.physics_uniform();
        queue.write_buffer(&self.physics_buffer, 0, bytemuck::cast_slice(&physics));

        self.live_pigment_latents = palette.pigment_latents();
        let base = pigment::GLOBAL_LATENTS + LIVE_PALETTE * pigment::PIGMENT_LATENTS;
        queue.write_buffer(
            &self.latents_buffer,
            (base * std::mem::size_of::<[f32; 4]>()) as u64,
            bytemuck::cast_slice(&self.live_pigment_latents),
        );
    }

    /// 乾かすとき、現行(live)パレットの顔料 latent を dried スロット `slot` の枠へ焼き込む(M5c)。
    /// これ以降に顔料を編集しても、このレイヤーは記録時の色のまま表示される
    fn record_layer_palette(&self, queue: &wgpu::Queue, slot: usize) {
        let base = pigment::GLOBAL_LATENTS + slot * pigment::PIGMENT_LATENTS;
        queue.write_buffer(
            &self.latents_buffer,
            (base * std::mem::size_of::<[f32; 4]>()) as u64,
            bytemuck::cast_slice(&self.live_pigment_latents),
        );
    }

    /// 「乾かす」(M2): 定着パスを1回走らせ、湿レイヤーの顔料を新しい乾燥レイヤーへ
    /// 焼き込んで湿レイヤーを解放する。手動ボタンから呼ばれる(フレーム外の即時 submit)
    pub fn bake_dry(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(), String> {
        let pipelines = self.pipelines.as_ref().ok_or("シェーダーが未ビルドです")?;
        if self.layers.len() >= MAX_LAYERS {
            return Err(format!(
                "乾燥レイヤーの上限({MAX_LAYERS}枚)に達しています。リセットしてください"
            ));
        }
        // スロットは焼き込み順に採番(解放は全消去のみなので len がそのまま次の空き)
        let slot = self.layers.len();
        // M5c: 現行パレットの色をこのレイヤー専用スロットへ焼き込む(以降の顔料編集で不変)
        self.record_layer_palette(queue, slot);

        let src = self.current;
        let dst = 1 - src;
        let mut entries = Vec::new();
        for (kind, pair) in self.sim_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: (kind * 2) as u32,
                resource: wgpu::BindingResource::TextureView(&pair[src]),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: (kind * 2 + 1) as u32,
                resource: wgpu::BindingResource::TextureView(&pair[dst]),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 6,
            resource: self.params_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 8,
            resource: wgpu::BindingResource::TextureView(&self.paper_view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 9,
            resource: wgpu::BindingResource::TextureView(&self.dried_slice_views[slot]),
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bake_bg"),
            layout: &self.bake_bgl,
            entries: &entries,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bake_encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("bake_pass"),
                timestamp_writes: None,
            });
            let workgroups = CANVAS_SIZE.div_ceil(8);
            pass.set_pipeline(pipelines.compute("bake.wgsl"));
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups, workgroups, 1);
        }
        queue.submit([encoder.finish()]);
        self.current = dst;

        self.layers.push(DriedLayer {
            slot: slot as u32,
            visible: true,
        });
        self.sync_layers(queue);
        Ok(())
    }

    /// Fast Dry(M2): 水だけ除去(浮遊顔料はその場で沈着)。焼き込みはしない
    pub fn fast_dry(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(), String> {
        self.run_oneshot(device, queue, "fastdry_pass", |p| p.compute("fastdry.wgsl"))
    }

    /// Wet the Layer(M2): キャンバス全面を再湿潤(水 += rewet_water、マスク=1)
    pub fn rewet(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(), String> {
        self.run_oneshot(device, queue, "rewet_pass", |p| p.compute("rewet.wgsl"))
    }

    /// 共通レイアウトの compute パスを1回だけ即時実行する(M2 の手動ボタン用)
    fn run_oneshot(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        pick: impl Fn(&Pipelines) -> &wgpu::ComputePipeline,
    ) -> Result<(), String> {
        let pipelines = self.pipelines.as_ref().ok_or("シェーダーが未ビルドです")?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(label),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            let workgroups = CANVAS_SIZE.div_ceil(8);
            pass.set_pipeline(pick(pipelines));
            pass.set_bind_group(0, &self.compute_bind_groups[self.current], &[]);
            pass.dispatch_workgroups(workgroups, workgroups, 1);
        }
        queue.submit([encoder.finish()]);
        self.current ^= 1;
        Ok(())
    }

    /// assets/shaders/ から WGSL を読み直してパイプラインを作り直す(H1)。
    /// common.wgsl(SimParams 等の共通定義)を各シェーダーの先頭に連結してコンパイルする。
    /// 失敗したら Err(表示用メッセージ) を返し、直前の正常なパイプラインを保持する。
    pub fn rebuild_pipelines(&mut self, device: &wgpu::Device) -> Result<(), String> {
        let read = |name: &str| {
            let path = self.shader_dir.join(name);
            std::fs::read_to_string(&path)
                .map_err(|e| format!("{} を読めません: {e}", path.display()))
        };
        let common = read("common.wgsl")?;
        // common.wgsl を先頭に連結する分、コンパイルエラーの行番号がずれる。
        // 補正のため連結プレフィックス(common + "\n")が占める行数を数えておく(R3)
        let prefix_lines = common.matches('\n').count() + 1;
        let load = |name: &str| -> Result<String, String> { Ok(format!("{common}\n{}", read(name)?)) };

        // エラースコープで検証エラーを捕捉し、クラッシュさせない
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let make_module = |label: &str, src: String| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
        };

        // compute パイプラインは COMPUTE_SHADERS の表を回して作る(シェーダー追加 = 表に1行。R3)
        let mut compute = HashMap::new();
        for &(name, layout) in COMPUTE_SHADERS {
            let module = make_module(name, load(name)?);
            let pipeline_layout = match layout {
                ComputeLayout::Common => &self.compute_layout,
                // bake(M2)だけ専用レイアウト(binding 9 = 乾燥レイヤースライス)
                ComputeLayout::Bake => &self.bake_layout,
                // ラスタ線画(M4.5a): 線画テクスチャ(read_write)専用レイアウト
                ComputeLayout::Raster => &self.raster_layout,
                // アクティブタイル(M6): タイル走査・膨張の専用レイアウト
                ComputeLayout::TileScan => &self.tilescan_layout,
                ComputeLayout::TileDilate => &self.tiledilate_layout,
            };
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(pipeline_layout),
                module: &module,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
            compute.insert(name, pipeline);
        }

        let display_module = make_module("display.wgsl", load("display.wgsl")?);
        // 同じ display シェーダーを、画面用とスナップショット用(H6)の2フォーマットで作る
        let make_display = |label: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&self.display_layout),
                vertex: wgpu::VertexState {
                    module: &display_module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &display_module,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let display = make_display("display_pipeline", self.target_format);
        let snapshot = make_display("snapshot_pipeline", self.snapshot_format);

        if let Some(error) = pollster::block_on(scope.pop()) {
            // common.wgsl 連結でずれた行番号をシェーダー内の行に補正して表示(R3 QoL)
            return Err(remap_shader_error_lines(&error.to_string(), prefix_lines));
        }
        self.pipelines = Some(Pipelines {
            compute,
            display,
            snapshot,
        });
        Ok(())
    }
}
