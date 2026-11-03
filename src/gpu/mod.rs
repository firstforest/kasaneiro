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
//! - フレームごとの描画は [`CanvasCallback`](callback)(prepare で compute、paint で表示)。
//!   compute 本体の `encode_frame` は headless 駆動([`crate::headless`]、e2e)からも呼ばれる
//! - ホットリロード時の再ビルドは src/app/mod.rs から rebuild_pipelines()
//!
//! ファイル構成(このモジュールはリソース定義と型・実行時メソッドを持つ。長い処理は分離):
//! - [`init`] — `GpuCanvas::new`(テクスチャ・バッファ・bind group の生成)
//! - [`brush`] — 塗る筆の中身(GPU の筆バッファ)と CPU への読み戻し
//! - [`callback`] — `CanvasCallback`(フレーム描画。パス実行順の正典)
//! - [`snapshot`] — `GpuCanvas::snapshot`(PNG 読み戻し。H6)
//! - [`persist`] — `GpuCanvas::export_state` / `import_state`(作品保存の GPU 読み戻し/書き戻し。M7)
//! - [`prelude`] — Rust と WGSL で共有する定数(MAX_LAYERS・latent 枠・タイル)と、それを
//!   WGSL の const として各シェーダー先頭に連結するプレリュードの生成
//! - [`shader_error`] — WGSL エラーの行番号補正

pub mod hot_reload;

mod brush;
mod callback;
mod init;
mod persist;
mod prelude;
mod shader_error;
mod snapshot;

pub use callback::{CanvasCallback, MixCallback};
pub use persist::WorkTextures;
pub use prelude::{LATENT_TOTAL, MAX_LAYERS};

/// 下地テクスチャのスライス数(0 = 乗算 / 1 = KM)
pub const BASE_SLICES: usize = 2;

/// 混色皿のキャンバス1辺(テクセル)。右パネルの幅に 1 テクセル ≒ 1 画面 px で収まる大きさ
pub const MIX_CANVAS_SIZE: u32 = 256;

/// 混色皿: 右パネルに置く小さな湿レイヤー。本体と同じシミュレーションを回し、筆の中身のバッファを
/// 本体と共有する(どちらで塗っても同じ筆が減り、吸う)。egui の callback_resources に
/// 本体の [`GpuCanvas`] と別の型で置くための包み
pub struct MixCanvas(pub GpuCanvas);

use prelude::{LIVE_PALETTE, PALETTE_SLOTS, TILE_SIZE, shader_prelude};

use eframe::egui_wgpu::wgpu;
use paint_core::sim::{MAX_SPLATS, SimParams, Splat, SplatHeader};
use shader_error::remap_shader_error_lines;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// シミュレーションテクスチャのフォーマット。レイアウトは common.wgsl のコメントと対応。
const SIM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// 乾燥レイヤーの合成キャッシュ。値は 0..1 の反射率・透過率なので半精度で足りる
const DRIED_CACHE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// ping-pong するテクスチャの種類数(水 / 浮遊顔料 / 沈着顔料)。
/// 紙ハイト(M1d)は静的なので含まない(paper_texture として別持ち)
const TEX_KINDS: usize = 3;

/// 紙ハイト生成のシード。「紙を作り直す」UI を足すならここをフィールド化する
const PAPER_SEED: u32 = 0x5EED;

/// 発散緩和の反復回数の上限(スライダー範囲より広めの安全弁)
const MAX_RELAX_ITERS: u32 = 64;

/// 顔料拡散の反復回数の上限(スライダー範囲より広めの安全弁)
const MAX_DIFFUSE_ITERS: u32 = 32;

/// compute シェーダーのバインドグループレイアウト種別。
/// ほとんどは共通レイアウト、bake だけ専用(乾燥レイヤースライス binding 9)。
#[derive(Clone, Copy)]
enum ComputeLayout {
    Common,
    Bake,
    /// ラスタ線画(M4.5a): 対象の線画テクスチャ(read_write)+ params + splats + 紙ハイト
    Raster,
    /// アクティブタイル走査(M6、tilescan): 水・浮遊顔料(read)+ params + splats + raw_active(write)
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

/// compute パイプラインの定義表。**シェーダー追加 = ここに1行**。
/// キーは WGSL ファイル名で、`Pipelines::compute()` の名前引きと prepare() の
/// パス実行順(ハードコード)が同じ名前を参照する。ここが持つのは「どのファイルを
/// どのレイアウトでビルドするか」だけで、実行順は心臓部なので prepare() 側に残す。
const COMPUTE_SHADERS: &[(&str, ComputeLayout)] = &[
    ("splat.wgsl", ComputeLayout::Common),
    ("brush.wgsl", ComputeLayout::Common),
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
    /// display.wgsl の dried_cache_main(乾燥レイヤーの合成キャッシュを焼く compute)
    dried_cache: wgpu::ComputePipeline,
}

impl Pipelines {
    /// WGSL ファイル名で compute パイプラインを引く。ビルド成功時は COMPUTE_SHADERS が
    /// 全て揃っているので、未登録名(タイプミス)は開発時に落として気付けるよう panic する
    fn compute(&self, name: &str) -> &wgpu::ComputePipeline {
        self.compute.get(name).unwrap_or_else(|| {
            panic!("compute パイプライン {name} が未登録です(COMPUTE_SHADERS を確認)")
        })
    }
}

/// 乾燥レイヤー(M2)1枚分のメタ情報。実体は dried_texture のスライス `slot`。
/// Vec の並び = 重ね順(先頭が最下層)。multiply 合成では順序は見た目に効かないが、
/// KM 合成(M3)で効くため UI の並べ替えをそのまま uniform へ流す
#[derive(Clone, Copy)]
pub struct DriedLayer {
    /// dried_texture のスライス番号(使用中は 0..layers.len()。重ね順とは独立)
    pub slot: u32,
    pub visible: bool,
    /// 合成時に顔料濃度へ掛ける倍率(1 = 乾かしたときのまま)。テクスチャは書き換えないので戻せる
    pub density: f32,
}

impl DriedLayer {
    /// 濃度倍率の UI 範囲
    pub const DENSITY_RANGE: std::ops::RangeInclusive<f32> = 0.0..=2.0;

    /// 乾かした直後の層(表示・倍率 1)
    pub fn new(slot: u32) -> Self {
        Self {
            slot,
            visible: true,
            density: 1.0,
        }
    }
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

/// display.wgsl の LayerUniform と同レイアウト(16 + MAX_LAYERS×8 バイト)
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    count: u32,
    /// bit k = 下から k 番目のレイヤーの可視性
    visible_mask: u32,
    /// 1 = 合成を紙ではなく下地テクスチャから始める
    has_base: u32,
    /// 0 = params.compose_mode に従う / 1 = 乗算 / 2 = KM(下地の統合で両方式を焼き分ける)
    force_mode: u32,
    /// order[k] = 下から k 番目のレイヤーのスロット番号
    order: [u32; MAX_LAYERS],
    /// density[k] = 下から k 番目のレイヤーの濃度倍率
    density: [f32; MAX_LAYERS],
}

/// リリース配布用(embed-assets): assets/shaders/ 全体をコンパイル時に埋め込む。
/// 通常ビルドは実行時ロード(H1 ホットリロード)のままにして試行錯誤の速度を守る
#[cfg(feature = "embed-assets")]
static EMBEDDED_SHADERS: include_dir::Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/shaders");

pub struct GpuCanvas {
    // embed-assets では実行時ロードしないため未読になる
    #[cfg_attr(feature = "embed-assets", allow(dead_code))]
    shader_dir: PathBuf,
    target_format: wgpu::TextureFormat,
    /// キャンバス1辺(テクセル)。全テクスチャ・dispatch・
    /// 読み戻しの寸法がここから決まる。サイズ変更はキャンバスの作り直し(app::recreate_canvas)
    size: u32,
    /// キャンバス1辺あたりのタイル数(= size / TILE_SIZE。M6 アクティブタイル)。
    /// WGSL 側の TILES_PER_SIDE(shader_prelude が生成)と一致する
    tiles_per_side: u32,
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
    /// 顔料個性 ρ/ω/γ/μ(M3。μ=粒の細かさは分離色で追加)。compute の binding 9。
    /// M5 で set_palette がランタイム書き替え
    physics_buffer: wgpu::Buffer,
    /// パン/ズーム(M6)。display の binding 11。フレームごとに CanvasCallback が書き替える
    view_buffer: wgpu::Buffer,
    /// 現行(live)パレットの顔料 latent ブロック。乾かすとき dried スロットへ焼き込む(M5c)
    live_pigment_latents: [[f32; 4]; pigment::PIGMENT_LATENTS],
    compute_layout: wgpu::PipelineLayout,
    display_layout: wgpu::PipelineLayout,
    /// 乾燥レイヤーの合成キャッシュを焼く bind group(キャッシュテクスチャ自体はここが保持する)
    dried_cache_bind_group: wgpu::BindGroup,
    dried_cache_layout: wgpu::PipelineLayout,
    /// 合成キャッシュの作り直しが必要か。乾燥レイヤーの中身・並び・可視性・記録パレット・
    /// シェーダーが変わったときに立てる。sync_layers が &self なので atomic にしている
    dried_cache_dirty: AtomicBool,
    /// 最後にキャッシュを焼いたときの (pigment_density のビット列, compose_mode)。
    /// SimParams のうちキャッシュに効くのはこの2つ
    dried_cache_key: Option<(u32, u32)>,
    /// 乾燥レイヤー(M2): スライス = レイヤースロット、rgba = 4顔料濃度。
    /// 作品保存(M7)で全スライスを読み戻し/書き戻しするため実体テクスチャを保持する
    dried_texture: wgpu::Texture,
    dried_slice_views: Vec<wgpu::TextureView>,
    /// 合成キャッシュの実体(下地への統合で下地テクスチャへ写す)
    dried_cache_texture: wgpu::Texture,
    /// 下地(2スライス: 0 = 乗算 / 1 = KM)。下の乾燥レイヤーを焼き固めた合成結果
    base_texture: wgpu::Texture,
    /// 下地が焼かれているか。false の間は合成が紙から始まり、下地テクスチャは読まれない
    pub(crate) has_base: bool,
    layers_buffer: wgpu::Buffer,
    bake_bgl: wgpu::BindGroupLayout,
    bake_layout: wgpu::PipelineLayout,
    /// 線画(M4.5a/c): [鉛筆, ペン, ハイライト] の r32float テクスチャ(read_write。ping-pong しない)。
    /// clear() / clear_line() でゼロに戻すため実体を保持する
    line_textures: [wgpu::Texture; 3],
    /// 清書ペンの下地: ストロークを描き始めたときのペン層の複製。linesplat.wgsl は描いている1本を
    /// これに重ねるので、1本の中は均一な濃さのまま、別のストロークと重なった所だけ濃くなる
    pen_base: wgpu::Texture,
    /// linesplat.wgsl 用の描画先別 bind group([鉛筆, ペン, ハイライト]。LineTarget::index の順)
    raster_bind_groups: [wgpu::BindGroup; 3],
    raster_layout: wgpu::PipelineLayout,
    /// アクティブタイル(M6): タイル走査/膨張の bind group。実体のバッファ
    /// (raw_active / active、array<u32, NUM_TILES>)は bind group が保持する。
    /// tilescan の bind group は current テクスチャを読むので ping-pong parity 別に2つ
    tilescan_bind_groups: [wgpu::BindGroup; 2],
    /// tiledilate の bind group(raw→active。バッファ固定なので1つ)
    tiledilate_bind_group: wgpu::BindGroup,
    /// ping-pong の2枚が食い違いうるタイルのフラグ(u32 × num_tiles)。
    /// 0 のタイルはシミュの各パスが書き込みごと省くので、2枚を揃えずに書き替える操作の後は
    /// [`GpuCanvas::mark_tiles_dirty`] で全タイル 1 にする
    tile_dirty_buffer: wgpu::Buffer,
    num_tiles: u32,
    tilescan_layout: wgpu::PipelineLayout,
    tiledilate_layout: wgpu::PipelineLayout,
    /// 乾燥レイヤーの重ね順とメタ情報(先頭が最下層)。UI(src/app/ui/layers.rs)から直接編集し、
    /// 変更後に sync_layers() で uniform へ反映する
    pub layers: Vec<DriedLayer>,
    /// 乾燥レイヤーの記録時パレット(M5h)。index = slot。GPU の latents 枠(M5c)が
    /// 色 latent しか持たないのに対し、こちらは名前・ρ/ω/γ 込みの**正典**
    /// (GPU 枠は表示用キャッシュ)。UI の「乾いた層からパレット抽出」が読む。
    /// 不変条件: `layer_palettes.len() == layers.len()` で、使用中のスロットは 0..len(添字 = slot)。
    /// bake_dry が末尾に push、clear が全消去、merge_into_base が残りを先頭へ詰め直し、
    /// 作品読込は work::decode が個数一致を検査した値を代入する
    pub layer_palettes: Vec<pigment::Palette>,
    /// 塗る筆の中身(正典は GPU。CPU は読み戻した値を表示と操作の起点に使う)
    brush: brush::GpuBrush,
    /// シェーダーが一度も通っていない/壊れている間は None(描画をスキップして継続)
    pipelines: Option<Pipelines>,
    /// 表示中のテクスチャ番号。各 compute パスは current を読み 1-current へ書いてから反転する
    current: usize,
    /// PNG スナップショット(H6)用: オフスクリーンターゲットと読み戻しバッファ
    snapshot_format: wgpu::TextureFormat,
    snapshot_texture: wgpu::Texture,
    snapshot_view: wgpu::TextureView,
    snapshot_buffer: wgpu::Buffer,
    /// デバイスで `Features::CLEAR_TEXTURE` が有効か(clear / clear_line の実装を選ぶ)
    clear_texture: bool,
}

impl GpuCanvas {
    /// キャンバス1辺(テクセル。M8)。app 側の座標変換・保存が参照する
    pub fn size(&self) -> u32 {
        self.size
    }

    /// キャンバスをリセット(水・速度・濡れマスク・顔料をゼロに = 乾いた白い紙)。
    /// 乾燥レイヤー(M2)も全て破棄する(スロットは焼き込み時に上書きされるため
    /// テクスチャ自体はクリア不要。count=0 で表示から消える)。
    /// 作成直後のテクスチャは wgpu がゼロ初期化するので、new() からは呼ばない
    pub fn clear(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let textures: Vec<&wgpu::Texture> = self
            .textures
            .iter()
            .flatten()
            .chain(self.line_textures.iter())
            .collect();
        self.zero_textures(device, queue, &textures);
        self.mark_tiles_dirty(queue);
        self.layers.clear();
        self.layer_palettes.clear();
        self.has_base = false;
        self.sync_layers(queue);
    }

    /// 2D テクスチャ(1 スライス・1 mip)群をゼロにする。`CLEAR_TEXTURE` が使えるデバイス
    /// (main.rs がアダプタ対応時に要求する)では GPU 上の clear_texture、使えなければ
    /// ゼロ埋めのバイト列を write_texture する(sim は rgba32float=16B、線画は r32float=4B)
    fn zero_textures(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        textures: &[&wgpu::Texture],
    ) {
        if self.clear_texture {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("clear_textures"),
            });
            for texture in textures {
                encoder.clear_texture(texture, &wgpu::ImageSubresourceRange::default());
            }
            queue.submit([encoder.finish()]);
            return;
        }
        let size = self.size;
        let max_texel = textures
            .iter()
            .filter_map(|t| t.format().block_copy_size(None))
            .max()
            .unwrap_or(0);
        let zeros = vec![0u8; (size * size * max_texel) as usize];
        for texture in textures {
            let texel = texture.format().block_copy_size(None).unwrap_or(0);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &zeros[..(size * size * texel) as usize],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * texel),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// 乾燥レイヤー slot の記録時パレット(M5h)。UI のパレット抽出が読む。
    /// 不変条件(len == layers.len())が保たれていれば使用スロットでは常に Some だが、
    /// 将来の変更(レイヤー個別削除など)で崩れたときの破綻検知を兼ねて Option のまま返す
    pub fn layer_palette(&self, slot: usize) -> Option<&pigment::Palette> {
        self.layer_palettes.get(slot)
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
        self.mark_tiles_dirty(queue);
    }

    /// backup(current→退避)/ restore(退避→current)の共通実装。`backup=true` で退避方向。
    fn copy_wet(&self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, backup: bool) {
        let extent = wgpu::Extent3d {
            width: self.size,
            height: self.size,
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
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
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

    /// 線画テクスチャ1枚をゼロに戻す(M4.5d の Undo で対象を再ラスタライズする前に呼ぶ)
    pub fn clear_line(&self, device: &wgpu::Device, queue: &wgpu::Queue, target: LineTarget) {
        self.zero_textures(device, queue, &[&self.line_textures[target.index()]]);
    }

    /// 清書ペンのストロークを描き始める前に、今のペン層を下地へ複製する
    pub fn snapshot_pen_base(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        fn at(texture: &wgpu::Texture) -> wgpu::TexelCopyTextureInfo<'_> {
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("snapshot_pen_base"),
        });
        encoder.copy_texture_to_texture(
            at(&self.line_textures[LineTarget::Pen.index()]),
            at(&self.pen_base),
            wgpu::Extent3d {
                width: self.size,
                height: self.size,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
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
        if target == LineTarget::Pen {
            self.snapshot_pen_base(device, queue);
        }
        let workgroups = self.size.div_ceil(8);
        for chunk in splats.chunks(MAX_SPLATS) {
            // 線画専用の即時実行なので tilescan 向けの fluid_count は 0
            let header = SplatHeader {
                count: chunk.len() as u32,
                fluid_count: 0,
                brush_dt: 0.0,
                _pad: 0,
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

    /// 筆の中身のバッファ。別のキャンバスを [`GpuCanvas::with_brush`] で作るときに渡すと同じ筆を使う
    pub fn brush_buffer(&self) -> wgpu::Buffer {
        self.brush.state.clone()
    }

    /// 同じ筆のバッファを使う別のキャンバス(混色皿)が筆の中身を書き換えうるパスを積んだ後に呼ぶ。
    /// 版を進めるだけで、読み戻しのコピーはこのキャンバスの encode_frame が積む。ここで積むと、
    /// 同じフレームの後で poll_brush が staging の map を始め、map 待ちのバッファを submit してしまう
    pub(crate) fn brush_touched_elsewhere(&mut self) {
        self.brush.touched();
    }

    /// 塗る筆の中身のうち最後に分かっている値(描いている間は数フレーム遅れる。表示用)
    pub fn brush_mirror(&self) -> paint_core::load::BrushLoad {
        self.brush.mirror()
    }

    /// 塗る筆の中身を置き換える(次の submit の前に効く)
    pub fn set_brush(&mut self, queue: &wgpu::Queue, load: paint_core::load::BrushLoad) {
        self.brush.set(queue, load);
    }

    /// 塗る筆の中身を GPU の完了を待って得る。CPU で中身を編集する前・記録に残す前に呼ぶ
    pub fn sync_brush(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<paint_core::load::BrushLoad, String> {
        self.brush.sync(device, queue)
    }

    /// 前のフレームまでに積んだ筆の中身の読み戻しを進める(完了は待たない)。
    /// フレームのコマンドを submit した後、次の encode_frame の前に呼ぶ
    pub fn poll_brush(&mut self, device: &wgpu::Device) {
        if let Err(e) = self.brush.poll(device, false) {
            log::warn!("{e}");
        }
    }

    /// 全タイルを「ping-pong の2枚が食い違いうる」にする。次にシミュが進むフレームで全タイルが
    /// 一度コピーされて2枚が揃う。current 側だけを書き替える操作(乾かす・Fast Dry・再湿潤・
    /// 湿レイヤーの undo・作品読込・全部消す)の後に呼ぶ。write_buffer なので次の submit の前に効く
    pub(crate) fn mark_tiles_dirty(&self, queue: &wgpu::Queue) {
        let ones = vec![1u32; self.num_tiles as usize];
        queue.write_buffer(&self.tile_dirty_buffer, 0, bytemuck::cast_slice(&ones));
    }

    /// レイヤーの並び・可視性(self.layers)を display 用 uniform へ反映する(M2)。
    /// レイヤーパネル(src/app/ui/layers.rs)が layers を編集したあとに呼ぶ
    pub fn sync_layers(&self, queue: &wgpu::Queue) {
        let uniform = self.layer_uniform(&self.layers, 0);
        queue.write_buffer(&self.layers_buffer, 0, bytemuck::bytes_of(&uniform));
        self.dried_cache_dirty.store(true, Ordering::Relaxed);
    }

    /// `layers`(先頭が最下層)を合成する LayerUniform。`force_mode` は LayerUniform の同名欄
    fn layer_uniform(&self, layers: &[DriedLayer], force_mode: u32) -> LayerUniform {
        let mut uniform = LayerUniform {
            count: layers.len().min(MAX_LAYERS) as u32,
            visible_mask: 0,
            has_base: self.has_base as u32,
            force_mode,
            order: [0; MAX_LAYERS],
            density: [1.0; MAX_LAYERS],
        };
        for (k, layer) in layers.iter().take(MAX_LAYERS).enumerate() {
            uniform.order[k] = layer.slot;
            uniform.density[k] = layer.density;
            if layer.visible {
                uniform.visible_mask |= 1 << k;
            }
        }
        uniform
    }

    /// 下から `count` 枚の乾燥レイヤーを下地に焼き固めて、そのスロットを空ける。
    /// 乗算・KM の両方式について「今の下地(なければ紙)+下の count 枚」の合成結果を焼くので
    /// 見た目は変わらない。代わりに焼いた層の並べ替え・表示切替・記録パレットは失われ、
    /// 非表示だった層は合成されずに消える。焼いた時点の紙色・顔料濃度・層の濃度倍率も下地に固まる。
    /// 残った層は重ね順のままスロット 0.. へ詰め直す(layer_palettes の添字 = slot を保つ)
    pub fn merge_into_base(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        count: usize,
    ) -> Result<(), String> {
        let pipelines = self.pipelines.as_ref().ok_or("シェーダーが未ビルドです")?;
        if count == 0 || count > self.layers.len() {
            return Err(format!("下地へ統合する層の数が不正です({count})"));
        }
        // 方式ごとに合成キャッシュへ焼いて下地のスライスへ写す。uniform の書き換えは次の submit の
        // 前に反映されるので、方式ごとに submit を分ける
        for (slice, force_mode) in [(0u32, 1u32), (1, 2)] {
            let uniform = self.layer_uniform(&self.layers[..count], force_mode);
            queue.write_buffer(&self.layers_buffer, 0, bytemuck::bytes_of(&uniform));
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("merge_base_encoder"),
            });
            self.encode_dried_cache(&mut encoder, pipelines);
            encoder.copy_texture_to_texture(
                self.dried_cache_texture.as_image_copy(),
                wgpu::TexelCopyTextureInfo {
                    texture: &self.base_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: slice,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: self.size,
                    height: self.size,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);
        }
        self.has_base = true;

        let remaining: Vec<DriedLayer> = self.layers[count..].to_vec();
        let old_palettes = std::mem::take(&mut self.layer_palettes);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("compact_layers_encoder"),
        });
        for (from, to) in compaction_moves(remaining.iter().map(|l| l.slot as usize)) {
            self.copy_dried_slice(&mut encoder, from, to);
        }
        queue.submit([encoder.finish()]);
        self.layers = remaining
            .iter()
            .enumerate()
            .map(|(j, l)| DriedLayer {
                slot: j as u32,
                ..*l
            })
            .collect();
        self.layer_palettes = remaining
            .iter()
            .map(|l| old_palettes[l.slot as usize].clone())
            .collect();
        // 記録パレットの GPU 枠も新しいスロットへ書き直す(CPU の記録パレットが正典)
        for (slot, palette) in self.layer_palettes.iter().enumerate() {
            self.write_layer_latents(queue, slot, &palette.pigment_latents());
        }
        self.sync_layers(queue);
        Ok(())
    }

    /// 乾燥テクスチャのスライス `from` を `to` へ写すコマンドを積む
    fn copy_dried_slice(&self, encoder: &mut wgpu::CommandEncoder, from: usize, to: usize) {
        let at = |z: usize| wgpu::TexelCopyTextureInfo {
            texture: &self.dried_texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: z as u32,
            },
            aspect: wgpu::TextureAspect::All,
        };
        encoder.copy_texture_to_texture(
            at(from),
            at(to),
            wgpu::Extent3d {
                width: self.size,
                height: self.size,
                depth_or_array_layers: 1,
            },
        );
    }

    /// 乾燥レイヤーの合成キャッシュを焼く compute パスを積む(display.wgsl の dried_cache_main)
    fn encode_dried_cache(&self, encoder: &mut wgpu::CommandEncoder, pipelines: &Pipelines) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("dried_cache_pass"),
            timestamp_writes: None,
        });
        let workgroups = self.size.div_ceil(8);
        pass.set_pipeline(&pipelines.dried_cache);
        pass.set_bind_group(0, &self.dried_cache_bind_group, &[]);
        pass.dispatch_workgroups(workgroups, workgroups, 1);
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
        self.write_layer_latents(queue, slot, &self.live_pigment_latents);
    }

    /// dried スロット `slot` の顔料 latent 枠を書く
    fn write_layer_latents(
        &self,
        queue: &wgpu::Queue,
        slot: usize,
        latents: &[[f32; 4]; pigment::PIGMENT_LATENTS],
    ) {
        let base = pigment::GLOBAL_LATENTS + slot * pigment::PIGMENT_LATENTS;
        queue.write_buffer(
            &self.latents_buffer,
            (base * std::mem::size_of::<[f32; 4]>()) as u64,
            bytemuck::cast_slice(latents),
        );
    }

    /// 「乾かす」(M2): 定着パスを1回走らせ、湿レイヤーの顔料を新しい乾燥レイヤーへ
    /// 焼き込んで湿レイヤーを解放する。手動ボタンから呼ばれる(フレーム外の即時 submit)。
    /// M5h: `palette` = 焼き込み時点の現行パレット。GPU の latent 焼き込み(色のみ)と対で
    /// CPU 側にも丸ごと記録する(layer_palettes の不変条件をこの関数内に閉じる)
    pub fn bake_dry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        palette: &pigment::Palette,
    ) -> Result<(), String> {
        let pipelines = self.pipelines.as_ref().ok_or("シェーダーが未ビルドです")?;
        if self.layers.len() >= MAX_LAYERS {
            return Err(format!(
                "乾燥レイヤーの上限({MAX_LAYERS}枚)に達しています。レイヤーパネルで下の層を下地に統合すると空きます"
            ));
        }
        // スロットは 0..len が使用中(下地への統合も残りを先頭へ詰め直す)なので len が次の空き
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
            let workgroups = self.size.div_ceil(8);
            pass.set_pipeline(pipelines.compute("bake.wgsl"));
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups, workgroups, 1);
        }
        queue.submit([encoder.finish()]);
        self.current = dst;
        self.mark_tiles_dirty(queue);

        self.layers.push(DriedLayer::new(slot as u32));
        // M5h: 記録時パレットの CPU 正典(index = slot = 今 push した layers の末尾と対)
        self.layer_palettes.push(palette.clone());
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
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            let workgroups = self.size.div_ceil(8);
            pass.set_pipeline(pick(pipelines));
            pass.set_bind_group(0, &self.compute_bind_groups[self.current], &[]);
            pass.dispatch_workgroups(workgroups, workgroups, 1);
        }
        queue.submit([encoder.finish()]);
        self.current ^= 1;
        self.mark_tiles_dirty(queue);
        Ok(())
    }

    /// WGSL ソースを1本ぶん取得する(通常ビルド: assets/shaders/ から実行時ロード。H1)
    #[cfg(not(feature = "embed-assets"))]
    fn shader_source(&self, name: &str) -> Result<String, String> {
        let path = self.shader_dir.join(name);
        std::fs::read_to_string(&path).map_err(|e| format!("{} を読めません: {e}", path.display()))
    }

    /// WGSL ソースを1本ぶん取得する(embed-assets: コンパイル時に埋め込んだものを返す)
    #[cfg(feature = "embed-assets")]
    fn shader_source(&self, name: &str) -> Result<String, String> {
        EMBEDDED_SHADERS
            .get_file(name)
            .and_then(|f| f.contents_utf8())
            .map(str::to_owned)
            .ok_or_else(|| format!("埋め込みシェーダー {name} がありません"))
    }

    /// WGSL を読み直してパイプラインを作り直す(H1。取得元は shader_source を参照)。
    /// common.wgsl(SimParams 等の共通定義)を各シェーダーの先頭に連結してコンパイルする。
    /// 失敗したら Err(表示用メッセージ) を返し、直前の正常なパイプラインを保持する。
    pub fn rebuild_pipelines(&mut self, device: &wgpu::Device) -> Result<(), String> {
        let read = |name: &str| self.shader_source(name);
        // キャンバスサイズ依存の定数(M8)を common.wgsl の前にさらに連結する。
        // 各シェーダー = プレリュード + common.wgsl + 本体、の3段
        let common = format!("{}{}", shader_prelude(self.size), read("common.wgsl")?);
        // 連結プレフィックスの分、コンパイルエラーの行番号がずれる。
        // 補正のため連結プレフィックス(プレリュード + common + "\n")の行数を数えておく
        let prefix_lines = common.matches('\n').count() + 1;
        let load =
            |name: &str| -> Result<String, String> { Ok(format!("{common}\n{}", read(name)?)) };
        // 読込失敗で早期 return してもエラースコープを pop し損ねないよう、
        // スコープを push する前に全シェーダーを読み切る
        let mut compute_sources = Vec::with_capacity(COMPUTE_SHADERS.len());
        for &(name, layout) in COMPUTE_SHADERS {
            compute_sources.push((name, layout, load(name)?));
        }
        let display_source = load("display.wgsl")?;

        // エラースコープで検証エラーを捕捉し、クラッシュさせない
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let make_module = |label: &str, src: String| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
        };

        // compute パイプラインは COMPUTE_SHADERS の表を回して作る(シェーダー追加 = 表に1行)
        let mut compute = HashMap::new();
        for (name, layout, source) in compute_sources {
            let module = make_module(name, source);
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

        let display_module = make_module("display.wgsl", display_source);
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
        let dried_cache = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("dried_cache_pipeline"),
            layout: Some(&self.dried_cache_layout),
            module: &display_module,
            entry_point: Some("dried_cache_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        if let Some(error) = pollster::block_on(scope.pop()) {
            // common.wgsl 連結でずれた行番号をシェーダー内の行に補正して表示
            return Err(remap_shader_error_lines(&error.to_string(), prefix_lines));
        }
        self.pipelines = Some(Pipelines {
            compute,
            display,
            snapshot,
            dried_cache,
        });
        self.dried_cache_dirty.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// スロット詰め直しのコピー手順。`slots[j]` = 残す層 j の今のスロットで、層 j をスロット j へ
/// 動かす (from, to) の列を返す。同じテクスチャ内のコピーなので、行き先にまだ動かしていない層が
/// いれば先に空きスロットへ逃がす(統合で1枚以上空くので空きは必ずある)
fn compaction_moves(slots: impl Iterator<Item = usize>) -> Vec<(usize, usize)> {
    let mut at: Vec<usize> = slots.collect();
    let mut holder: [Option<usize>; MAX_LAYERS] = [None; MAX_LAYERS];
    for (j, &s) in at.iter().enumerate() {
        holder[s] = Some(j);
    }
    let mut moves = Vec::new();
    for j in 0..at.len() {
        if at[j] == j {
            continue;
        }
        // スロット j にいるのは、まだ動かしていない層(i > j)か、誰もいないか
        if let Some(i) = holder[j] {
            let free = (j + 1..MAX_LAYERS)
                .find(|&f| holder[f].is_none())
                .expect("統合でスロットが空いている");
            moves.push((j, free));
            holder[free] = Some(i);
            at[i] = free;
        }
        moves.push((at[j], j));
        holder[at[j]] = None;
        holder[j] = Some(j);
        at[j] = j;
    }
    moves
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手順どおりに写すと、どの並びでも層 j がスロット j に来る
    #[test]
    fn compaction_moves_place_each_layer() {
        let cases: [&[usize]; 5] = [
            &[3, 4, 5],
            &[1, 0],
            &[7, 2, 0, 5],
            &[0, 1, 2],
            &[2, 0, 1, 6, 5, 4, 3],
        ];
        for slots in cases {
            // 中身 = 層番号(None = 空き)
            let mut tex: [Option<usize>; MAX_LAYERS] = [None; MAX_LAYERS];
            for (j, &s) in slots.iter().enumerate() {
                tex[s] = Some(j);
            }
            for (from, to) in compaction_moves(slots.iter().copied()) {
                assert_ne!(from, to);
                tex[to] = tex[from];
            }
            // 未移動の層を潰していれば、その層はどこにも残らず下の一致が崩れる
            let want: Vec<Option<usize>> = (0..slots.len()).map(Some).collect();
            assert_eq!(tex[..slots.len()], want[..], "{slots:?}");
        }
    }
}
