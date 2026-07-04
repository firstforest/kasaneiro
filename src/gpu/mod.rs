//! wgpu リソース管理: シミュレーションテクスチャ(水 / 浮遊顔料 / 沈着顔料、
//! いずれも rgba32float の ping-pong ペア。加えて紙ハイト r32float 1枚=静的)、
//! compute パス群(splat / 速度更新 / 発散緩和 / FlowOutward / 移流 / 拡散 /
//! 吸着・脱着+蒸発)、表示用パイプライン、PNG スナップショット用の読み戻し(H6)、
//! WGSL の実行時ロードと再ビルド(H1)。
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
    display: wgpu::RenderPipeline,
    /// display と同じシェーダーを PNG スナップショット用フォーマットで焼くパイプライン(H6)
    snapshot: wgpu::RenderPipeline,
}

pub struct GpuCanvas {
    shader_dir: PathBuf,
    target_format: wgpu::TextureFormat,
    /// [水, 浮遊顔料, 沈着顔料] × ping-pong 2枚
    textures: [[wgpu::Texture; 2]; TEX_KINDS],
    compute_bind_groups: [wgpu::BindGroup; 2],
    display_bind_groups: [wgpu::BindGroup; 2],
    params_buffer: wgpu::Buffer,
    splat_buffer: wgpu::Buffer,
    compute_layout: wgpu::PipelineLayout,
    display_layout: wgpu::PipelineLayout,
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
        let views: Vec<[wgpu::TextureView; 2]> = textures
            .iter()
            .map(|pair| {
                [
                    pair[0].create_view(&wgpu::TextureViewDescriptor::default()),
                    pair[1].create_view(&wgpu::TextureViewDescriptor::default()),
                ]
            })
            .collect();

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
        // + 紙ハイト(M1d、表示モード 6)
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
            ],
        });

        // src=current / dst=もう片方 の2方向分
        let make_compute_bg = |src: usize, dst: usize| {
            let mut entries = Vec::new();
            for (kind, pair) in views.iter().enumerate() {
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
            for (kind, pair) in views.iter().enumerate() {
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

        let canvas = Self {
            shader_dir,
            target_format,
            textures,
            compute_bind_groups,
            display_bind_groups,
            params_buffer,
            splat_buffer,
            compute_layout,
            display_layout,
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

    /// キャンバスをリセット(水・速度・濡れマスク・顔料をゼロに = 乾いた白い紙)
    pub fn clear(&self, queue: &wgpu::Queue) {
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
