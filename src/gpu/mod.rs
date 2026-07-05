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
//! - フレームごとの描画は CanvasCallback(prepare で compute、paint で表示)
//! - ホットリロード時の再ビルドは app.rs から rebuild_pipelines()

pub mod hot_reload;

use crate::paper;
use crate::pigment;
use crate::sim::{CANVAS_SIZE, MAX_SPLATS, SimParams, Splat, SplatHeader};
use eframe::egui_wgpu::{self, wgpu};
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

/// 発散緩和の反復回数の上限(スライダー範囲より広めの安全弁)
const MAX_RELAX_ITERS: u32 = 64;

/// 顔料拡散の反復回数の上限(スライダー範囲より広めの安全弁)
const MAX_DIFFUSE_ITERS: u32 = 32;

struct Pipelines {
    splat: wgpu::ComputePipeline,
    velocity: wgpu::ComputePipeline,
    relax: wgpu::ComputePipeline,
    flowout: wgpu::ComputePipeline,
    advect: wgpu::ComputePipeline,
    diffuse: wgpu::ComputePipeline,
    transfer: wgpu::ComputePipeline,
    /// M2「乾かす」= 定着パス(専用レイアウト: 共通 0..6,8 + 乾燥レイヤースライス 9)
    bake: wgpu::ComputePipeline,
    /// M2 Fast Dry(水だけ除去。共通レイアウト)
    fastdry: wgpu::ComputePipeline,
    /// M2 Wet the Layer(全面再湿潤。共通レイアウト)
    rewet: wgpu::ComputePipeline,
    display: wgpu::RenderPipeline,
    /// display と同じシェーダーを PNG スナップショット用フォーマットで焼くパイプライン(H6)
    snapshot: wgpu::RenderPipeline,
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
    paper_view: wgpu::TextureView,
    compute_bind_groups: [wgpu::BindGroup; 2],
    display_bind_groups: [wgpu::BindGroup; 2],
    params_buffer: wgpu::Buffer,
    splat_buffer: wgpu::Buffer,
    compute_layout: wgpu::PipelineLayout,
    display_layout: wgpu::PipelineLayout,
    /// 乾燥レイヤー(M2): スライス = レイヤースロット、rgba = 4顔料濃度
    dried_slice_views: Vec<wgpu::TextureView>,
    layers_buffer: wgpu::Buffer,
    bake_bgl: wgpu::BindGroupLayout,
    bake_layout: wgpu::PipelineLayout,
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
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
        shader_dir: PathBuf,
    ) -> Self {
        let make_texture = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: CANVAS_SIZE,
                    height: CANVAS_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SIM_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let textures = [
            [make_texture("water_a"), make_texture("water_b")],
            [make_texture("susp_a"), make_texture("susp_b")],
            [make_texture("dep_a"), make_texture("dep_b")],
        ];
        let sim_views: [[wgpu::TextureView; 2]; TEX_KINDS] = std::array::from_fn(|kind| {
            std::array::from_fn(|i| {
                textures[kind][i].create_view(&wgpu::TextureViewDescriptor::default())
            })
        });

        // 乾燥レイヤー(M2): texture array 1枚に MAX_LAYERS スライス。
        // 作成直後は wgpu がゼロ初期化する = 濃度ゼロ = multiply で無色(白)
        let dried_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dried_layers"),
            size: wgpu::Extent3d {
                width: CANVAS_SIZE,
                height: CANVAS_SIZE,
                depth_or_array_layers: MAX_LAYERS as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SIM_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        // 表示用: 全スライスを 2d array で / 焼き込み用: スライスごとの 2d ビュー
        let dried_array_view = dried_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let dried_slice_views: Vec<wgpu::TextureView> = (0..MAX_LAYERS as u32)
            .map(|slot| {
                dried_texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: slot,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let layers_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dried_layer_uniform"),
            size: std::mem::size_of::<LayerUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // 紙ハイト(M1d): CPU 生成の静的テクスチャ。ping-pong せず全パスから読み専用
        let paper_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("paper_height"),
            size: wgpu::Extent3d {
                width: CANVAS_SIZE,
                height: CANVAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let heights = paper::generate(CANVAS_SIZE, PAPER_SEED);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &paper_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&heights),
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
        let paper_view = paper_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // PNG スナップショット(H6): 画面と同じ見た目になるよう sRGB か否かを表示先に合わせる
        let snapshot_format = if target_format.is_srgb() {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let snapshot_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("snapshot"),
            size: wgpu::Extent3d {
                width: CANVAS_SIZE,
                height: CANVAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: snapshot_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let snapshot_view = snapshot_texture.create_view(&wgpu::TextureViewDescriptor::default());
        // rgba8 1 行 = CANVAS_SIZE×4 バイト。512 なら 2048 で copy の 256 バイト整列を満たす
        let snapshot_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("snapshot_readback"),
            size: (CANVAS_SIZE * CANVAS_SIZE * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sim_params"),
            size: std::mem::size_of::<SimParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let splat_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splats"),
            size: (std::mem::size_of::<SplatHeader>()
                + MAX_SPLATS * std::mem::size_of::<Splat>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // 顔料+紙色の mixbox latent(M1c)。パレットは固定なので起動時に1回書くだけ
        let latents = pigment::latent_uniform();
        let pigment_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pigment_latents"),
            size: std::mem::size_of_val(&latents) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&pigment_buffer, 0, bytemuck::cast_slice(&latents));

        let sampled_entry = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                // rgba32float はフィルタ不可(シェーダー側は textureLoad で読む)
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: SIM_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let buffer_entry = |binding: u32,
                            visibility: wgpu::ShaderStages,
                            ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        // 全 compute パス共通のレイアウト。binding は common.wgsl のコメントと対応:
        // 0/1 = 水 src/dst, 2/3 = 浮遊 src/dst, 4/5 = 沈着 src/dst, 6 = params, 7 = splats,
        // 8 = 紙ハイト(M1d、静的)
        let compute_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sim_bgl"),
            entries: &[
                sampled_entry(0, wgpu::ShaderStages::COMPUTE),
                storage_entry(1),
                sampled_entry(2, wgpu::ShaderStages::COMPUTE),
                storage_entry(3),
                sampled_entry(4, wgpu::ShaderStages::COMPUTE),
                storage_entry(5),
                buffer_entry(
                    6,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Uniform,
                ),
                buffer_entry(
                    7,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                sampled_entry(8, wgpu::ShaderStages::COMPUTE),
            ],
        });

        // 表示は 3 テクスチャ + params(H4 の表示モード分岐)+ 顔料 latent(M1c の mixbox 混色)
        // + 紙ハイト(M1d、表示モード 6)+ 乾燥レイヤー array + レイヤー uniform(M2)
        let display_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("display_bgl"),
            entries: &[
                sampled_entry(0, wgpu::ShaderStages::FRAGMENT),
                sampled_entry(1, wgpu::ShaderStages::FRAGMENT),
                sampled_entry(2, wgpu::ShaderStages::FRAGMENT),
                buffer_entry(
                    3,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BufferBindingType::Uniform,
                ),
                buffer_entry(
                    4,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BufferBindingType::Uniform,
                ),
                sampled_entry(5, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                buffer_entry(
                    7,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BufferBindingType::Uniform,
                ),
            ],
        });

        // bake(M2)専用レイアウト: 共通の 0..6, 8 + 乾燥レイヤースライス 9(splats の 7 は不要)。
        // 書き込み先スライスが焼き込みごとに変わるため、bind group はボタン押下時に組む
        let bake_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bake_bgl"),
            entries: &[
                sampled_entry(0, wgpu::ShaderStages::COMPUTE),
                storage_entry(1),
                sampled_entry(2, wgpu::ShaderStages::COMPUTE),
                storage_entry(3),
                sampled_entry(4, wgpu::ShaderStages::COMPUTE),
                storage_entry(5),
                buffer_entry(
                    6,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Uniform,
                ),
                sampled_entry(8, wgpu::ShaderStages::COMPUTE),
                storage_entry(9),
            ],
        });

        // src=current / dst=もう片方 の2方向分
        let make_compute_bg = |src: usize, dst: usize| {
            let mut entries = Vec::new();
            for (kind, pair) in sim_views.iter().enumerate() {
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
                resource: params_buffer.as_entire_binding(),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 7,
                resource: splat_buffer.as_entire_binding(),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&paper_view),
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sim_bg"),
                layout: &compute_bgl,
                entries: &entries,
            })
        };
        let compute_bind_groups = [make_compute_bg(0, 1), make_compute_bg(1, 0)];

        let make_display_bg = |i: usize| {
            let mut entries = Vec::new();
            for (kind, pair) in sim_views.iter().enumerate() {
                entries.push(wgpu::BindGroupEntry {
                    binding: kind as u32,
                    resource: wgpu::BindingResource::TextureView(&pair[i]),
                });
            }
            entries.push(wgpu::BindGroupEntry {
                binding: 3,
                resource: params_buffer.as_entire_binding(),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 4,
                resource: pigment_buffer.as_entire_binding(),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&paper_view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&dried_array_view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 7,
                resource: layers_buffer.as_entire_binding(),
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("display_bg"),
                layout: &display_bgl,
                entries: &entries,
            })
        };
        let display_bind_groups = [make_display_bg(0), make_display_bg(1)];

        let compute_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sim_pipeline_layout"),
            bind_group_layouts: &[Some(&compute_bgl)],
            immediate_size: 0,
        });
        let display_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("display_pipeline_layout"),
            bind_group_layouts: &[Some(&display_bgl)],
            immediate_size: 0,
        });
        let bake_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bake_pipeline_layout"),
            bind_group_layouts: &[Some(&bake_bgl)],
            immediate_size: 0,
        });

        let mut canvas = Self {
            shader_dir,
            target_format,
            textures,
            sim_views,
            paper_view,
            compute_bind_groups,
            display_bind_groups,
            params_buffer,
            splat_buffer,
            compute_layout,
            display_layout,
            dried_slice_views,
            layers_buffer,
            bake_bgl,
            bake_layout,
            layers: Vec::new(),
            pipelines: None,
            current: 0,
            snapshot_format,
            snapshot_texture,
            snapshot_view,
            snapshot_buffer,
        };
        canvas.clear(queue);
        canvas
    }

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
        self.layers.clear();
        self.sync_layers(queue);
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
            pass.set_pipeline(&pipelines.bake);
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
        self.run_oneshot(device, queue, "fastdry_pass", |p| &p.fastdry)
    }

    /// Wet the Layer(M2): キャンバス全面を再湿潤(水 += rewet_water、マスク=1)
    pub fn rewet(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(), String> {
        self.run_oneshot(device, queue, "rewet_pass", |p| &p.rewet)
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
        let load = |name: &str| -> Result<String, String> {
            // エラーメッセージの行番号は common.wgsl の行数分ずれる点に注意
            Ok(format!("{common}\n{}", read(name)?))
        };
        let splat_src = load("splat.wgsl")?;
        let velocity_src = load("velocity.wgsl")?;
        let relax_src = load("relax.wgsl")?;
        let flowout_src = load("flowout.wgsl")?;
        let advect_src = load("advect.wgsl")?;
        let diffuse_src = load("diffuse.wgsl")?;
        let transfer_src = load("transfer.wgsl")?;
        let bake_src = load("bake.wgsl")?;
        let fastdry_src = load("fastdry.wgsl")?;
        let rewet_src = load("rewet.wgsl")?;
        let display_src = load("display.wgsl")?;

        // エラースコープで検証エラーを捕捉し、クラッシュさせない
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let make_module = |label: &str, src: String| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
        };
        let make_compute = |label: &str, module: &wgpu::ShaderModule| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&self.compute_layout),
                module,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };

        let splat = make_compute("splat_pipeline", &make_module("splat.wgsl", splat_src));
        let velocity = make_compute(
            "velocity_pipeline",
            &make_module("velocity.wgsl", velocity_src),
        );
        let relax = make_compute("relax_pipeline", &make_module("relax.wgsl", relax_src));
        let flowout = make_compute(
            "flowout_pipeline",
            &make_module("flowout.wgsl", flowout_src),
        );
        let advect = make_compute("advect_pipeline", &make_module("advect.wgsl", advect_src));
        let diffuse = make_compute(
            "diffuse_pipeline",
            &make_module("diffuse.wgsl", diffuse_src),
        );
        let transfer = make_compute(
            "transfer_pipeline",
            &make_module("transfer.wgsl", transfer_src),
        );
        let fastdry = make_compute(
            "fastdry_pipeline",
            &make_module("fastdry.wgsl", fastdry_src),
        );
        let rewet = make_compute("rewet_pipeline", &make_module("rewet.wgsl", rewet_src));
        // bake(M2)だけ専用レイアウト(binding 9 = 乾燥レイヤースライス)
        let bake = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("bake_pipeline"),
            layout: Some(&self.bake_layout),
            module: &make_module("bake.wgsl", bake_src),
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let display_module = make_module("display.wgsl", display_src);
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
            return Err(error.to_string());
        }
        self.pipelines = Some(Pipelines {
            splat,
            velocity,
            relax,
            flowout,
            advect,
            diffuse,
            transfer,
            bake,
            fastdry,
            rewet,
            display,
            snapshot,
        });
        Ok(())
    }

    /// 現在のキャンバス表示を RGBA8(行連続、CANVAS_SIZE²)で読み戻す(H6 PNG スナップショット)。
    /// display と同じシェーダー・同じ表示モードで焼くため、画面に見えているものがそのまま残る。
    /// GPU 完了を同期で待つ(手動ボタン操作なので 1 フレームの停止は許容)。
    pub fn snapshot(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Vec<u8>, String> {
        let pipelines = self
            .pipelines
            .as_ref()
            .ok_or("シェーダーが未ビルドです")?;

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("snapshot_encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("snapshot_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.snapshot_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipelines.snapshot);
            pass.set_bind_group(0, &self.display_bind_groups[self.current], &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            self.snapshot_texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.snapshot_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(CANVAS_SIZE * 4),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: CANVAS_SIZE,
                height: CANVAS_SIZE,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = self.snapshot_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|e| format!("GPU の完了待ちに失敗: {e:?}"))?;
        rx.recv()
            .map_err(|e| format!("map コールバックが届きません: {e}"))?
            .map_err(|e| format!("読み戻しバッファの map に失敗: {e:?}"))?;
        let data = slice.get_mapped_range().to_vec();
        self.snapshot_buffer.unmap();
        Ok(data)
    }
}

/// 1フレーム分の描画データ。app.rs で組み立てて PaintCallback として渡す。
pub struct CanvasCallback {
    pub params: SimParams,
    pub splats: Vec<Splat>,
    /// このフレームで進めるシミュレーションステップ数(H6: 0=一時停止中)
    pub sim_steps: u32,
}

impl egui_wgpu::CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(canvas) = callback_resources.get_mut::<GpuCanvas>() else {
            return Vec::new();
        };
        queue.write_buffer(&canvas.params_buffer, 0, bytemuck::bytes_of(&self.params));

        let splat_count = self.splats.len().min(MAX_SPLATS);
        if splat_count > 0 {
            let header = SplatHeader {
                count: splat_count as u32,
                _pad: [0; 3],
            };
            let mut bytes = Vec::with_capacity(
                std::mem::size_of::<SplatHeader>()
                    + splat_count * std::mem::size_of::<Splat>(),
            );
            bytes.extend_from_slice(bytemuck::bytes_of(&header));
            bytes.extend_from_slice(bytemuck::cast_slice(&self.splats[..splat_count]));
            queue.write_buffer(&canvas.splat_buffer, 0, &bytes);
        }

        let mut current = canvas.current;
        if let Some(pipelines) = &canvas.pipelines {
            let workgroups = CANVAS_SIZE.div_ceil(8);
            let bind_groups = &canvas.compute_bind_groups;
            let mut pass = egui_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sim_pass"),
                timestamp_writes: None,
            });
            // 1 dispatch = current を読み、もう片方へ書き、反転(ping-pong)
            let mut run = |pass: &mut wgpu::ComputePass, pipeline: &wgpu::ComputePipeline| {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind_groups[current], &[]);
                pass.dispatch_workgroups(workgroups, workgroups, 1);
                current ^= 1;
            };

            // ブラシ入力(水+初速+顔料の注入)は一時停止中でも反映する
            if splat_count > 0 {
                run(&mut pass, &pipelines.splat);
            }
            // 1 ステップ = 速度更新 → 発散緩和 × N → FlowOutward → 移流
            //   → 顔料拡散 × N → 吸着/脱着+蒸発
            let relax_iters = self.params.relax_iters.clamp(1, MAX_RELAX_ITERS);
            let diffuse_iters = self.params.diffuse_iters.min(MAX_DIFFUSE_ITERS);
            for _ in 0..self.sim_steps {
                run(&mut pass, &pipelines.velocity);
                for _ in 0..relax_iters {
                    run(&mut pass, &pipelines.relax);
                }
                // エッジダークニング(M1d)。η=0 ならぼかしの読み出しごと省略
                if self.params.edge_eta > 0.0 {
                    run(&mut pass, &pipelines.flowout);
                }
                run(&mut pass, &pipelines.advect);
                for _ in 0..diffuse_iters {
                    run(&mut pass, &pipelines.diffuse);
                }
                run(&mut pass, &pipelines.transfer);
            }
        }
        canvas.current = current;
        Vec::new()
    }

    fn paint(
        &self,
        _info: eframe::egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(canvas) = callback_resources.get::<GpuCanvas>() else {
            return;
        };
        let Some(pipelines) = &canvas.pipelines else {
            return;
        };
        // egui-wgpu がビューポートをコールバック矩形に設定済みなので、
        // フルスクリーン三角形がそのままキャンバス領域に収まる
        render_pass.set_pipeline(&pipelines.display);
        render_pass.set_bind_group(0, &canvas.display_bind_groups[canvas.current], &[]);
        render_pass.draw(0..3, 0..1);
    }
}
