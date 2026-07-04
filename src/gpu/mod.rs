//! wgpu リソース管理: キャンバステクスチャ(ping-pong)、splat compute パス、
//! 表示用パイプライン、WGSL の実行時ロードと再ビルド(H1)。
//!
//! GpuCanvas は egui-wgpu の callback_resources に置かれ、
//! - フレームごとの描画は CanvasCallback(prepare で compute、paint で表示)
//! - ホットリロード時の再ビルドは app.rs から rebuild_pipelines()
//! の2経路から触られる。

pub mod hot_reload;

use crate::sim::{CANVAS_SIZE, MAX_SPLATS, SimParams, Splat, SplatHeader};
use eframe::egui_wgpu::{self, wgpu};
use std::path::PathBuf;

struct Pipelines {
    compute: wgpu::ComputePipeline,
    display: wgpu::RenderPipeline,
}

pub struct GpuCanvas {
    shader_dir: PathBuf,
    target_format: wgpu::TextureFormat,
    textures: [wgpu::Texture; 2],
    compute_bind_groups: [wgpu::BindGroup; 2],
    display_bind_groups: [wgpu::BindGroup; 2],
    params_buffer: wgpu::Buffer,
    splat_buffer: wgpu::Buffer,
    compute_layout: wgpu::PipelineLayout,
    display_layout: wgpu::PipelineLayout,
    /// シェーダーが一度も通っていない/壊れている間は None(描画をスキップして継続)
    pipelines: Option<Pipelines>,
    /// 表示中のテクスチャ番号。compute は current を読み 1-current へ書いてから反転する
    current: usize,
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
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let textures = [make_texture("canvas_a"), make_texture("canvas_b")];
        let views = [
            textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];

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

        let compute_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let display_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("display_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // src=current / dst=もう片方 の2方向分
        let make_compute_bg = |src: usize, dst: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("splat_bg"),
                layout: &compute_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[src]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&views[dst]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: splat_buffer.as_entire_binding(),
                    },
                ],
            })
        };
        let compute_bind_groups = [make_compute_bg(0, 1), make_compute_bg(1, 0)];

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("canvas_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let make_display_bg = |i: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("display_bg"),
                layout: &display_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        let display_bind_groups = [make_display_bg(0), make_display_bg(1)];

        let compute_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("splat_pipeline_layout"),
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
        };
        canvas.clear(queue);
        canvas
    }

    /// キャンバスを紙の白で塗りつぶす
    pub fn clear(&self, queue: &wgpu::Queue) {
        let white = vec![0xFFu8; (CANVAS_SIZE * CANVAS_SIZE * 4) as usize];
        for texture in &self.textures {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &white,
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
    }

    /// assets/shaders/ から WGSL を読み直してパイプラインを作り直す(H1)。
    /// 失敗したら Err(表示用メッセージ) を返し、直前の正常なパイプラインを保持する。
    pub fn rebuild_pipelines(&mut self, device: &wgpu::Device) -> Result<(), String> {
        let read = |name: &str| {
            let path = self.shader_dir.join(name);
            std::fs::read_to_string(&path)
                .map_err(|e| format!("{} を読めません: {e}", path.display()))
        };
        let splat_src = read("splat.wgsl")?;
        let display_src = read("display.wgsl")?;

        // エラースコープで検証エラーを捕捉し、クラッシュさせない
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let splat_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("splat.wgsl"),
            source: wgpu::ShaderSource::Wgsl(splat_src.into()),
        });
        let display_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("display.wgsl"),
            source: wgpu::ShaderSource::Wgsl(display_src.into()),
        });

        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("splat_pipeline"),
            layout: Some(&self.compute_layout),
            module: &splat_module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let display = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("display_pipeline"),
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
                    format: self.target_format,
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
        });

        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(error.to_string());
        }
        self.pipelines = Some(Pipelines { compute, display });
        Ok(())
    }
}

/// 1フレーム分の描画データ。app.rs で組み立てて PaintCallback として渡す。
pub struct CanvasCallback {
    pub params: SimParams,
    pub splats: Vec<Splat>,
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

        if !self.splats.is_empty() {
            if let Some(pipelines) = &canvas.pipelines {
                let count = self.splats.len().min(MAX_SPLATS);
                let header = SplatHeader {
                    count: count as u32,
                    _pad: [0; 3],
                };
                let mut bytes =
                    Vec::with_capacity(std::mem::size_of::<SplatHeader>() + count * 16);
                bytes.extend_from_slice(bytemuck::bytes_of(&header));
                bytes.extend_from_slice(bytemuck::cast_slice(&self.splats[..count]));
                queue.write_buffer(&canvas.splat_buffer, 0, &bytes);

                let mut pass = egui_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("splat_pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&pipelines.compute);
                pass.set_bind_group(0, &canvas.compute_bind_groups[canvas.current], &[]);
                let workgroups = CANVAS_SIZE.div_ceil(8);
                pass.dispatch_workgroups(workgroups, workgroups, 1);
                drop(pass);

                canvas.current ^= 1;
            }
        }
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
