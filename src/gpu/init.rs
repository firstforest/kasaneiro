//! GpuCanvas の初期化(リソース生成)。gpu/mod.rs から分離。
//!
//! テクスチャ(シミュ ping-pong・乾燥レイヤー array・紙ハイト・スナップショット)、
//! バッファ(params/splats/顔料 latent/顔料個性/レイヤー uniform)、bind group layout と
//! bind group、pipeline layout をまとめて確保して `GpuCanvas` を組み立てる。パイプライン本体は
//! WGSL の実行時ロード(H1)に委ねるため、ここでは `pipelines: None` で始める。

use super::{GpuCanvas, LayerUniform, MAX_LAYERS, PAPER_SEED, SIM_FORMAT, TEX_KINDS, ViewUniform};
use paint_core::paper;
use paint_core::sim::{CANVAS_SIZE, MAX_SPLATS, SimParams, Splat, SplatHeader};
use eframe::egui_wgpu::wgpu;
use std::path::PathBuf;

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
                    // COPY_SRC は湿レイヤーの undo 退避(M6)でテクスチャ間コピーの読み元にするため
                    | wgpu::TextureUsages::COPY_SRC
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

        // 湿レイヤーの 1 段 undo(M6): ストローク開始時に current の 3 テクスチャ(水 / 浮遊 /
        // 沈着)をここへ GPU 間コピーで退避し、Ctrl+Z で current へ書き戻す。表示専用でなく
        // コピーの読み元/書き先にしかならないので usage は COPY のみ(512² rgba32float × 3 = 12MB)
        let wet_backup: [wgpu::Texture; TEX_KINDS] = std::array::from_fn(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("wet_backup"),
                size: wgpu::Extent3d {
                    width: CANVAS_SIZE,
                    height: CANVAS_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SIM_FORMAT,
                usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
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

        // 線画(M4.5a): 鉛筆・ペン各 r32float 1枚。ping-pong せず、linesplat.wgsl が
        // read_write storage で直接インクを蓄積する。表示側は sampled として読む
        let make_line_texture = |label: &str| {
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
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        // [鉛筆, ペン, ハイライト] の順(LineTarget::index と一致)。ハイライトは M4.5c
        let line_textures = [
            make_line_texture("line_pencil"),
            make_line_texture("line_pen"),
            make_line_texture("line_highlight"),
        ];
        let line_views: [wgpu::TextureView; 3] = std::array::from_fn(|i| {
            line_textures[i].create_view(&wgpu::TextureViewDescriptor::default())
        });

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
        // 顔料+紙色の mixbox latent(M1c/M5c)。レイアウト = グローバル光学(紙/白/黒)+
        // パレット数 × 顔料ブロック。パレット枠は「乾燥レイヤー MAX_LAYERS 個 + 現行(live)1個」。
        // 乾かすたびに現行パレットの顔料ブロックを対応スロットへ焼き込む(M5c: レイヤーごとパレット
        // 記録)ため、顔料を後から編集しても乾燥済みレイヤーの色は変わらない。起動時は既定パレットで
        // 全スロットを埋める(display.wgsl の array<vec4f, LATENT_TOTAL> と対応)
        let palette = pigment::Palette::default_palette();
        let live_pigment_latents = palette.pigment_latents();
        let mut latents = [[0.0f32; 4]; super::LATENT_TOTAL];
        latents[..pigment::GLOBAL_LATENTS].copy_from_slice(&pigment::global_latents());
        for pal in 0..super::PALETTE_SLOTS {
            let base = pigment::GLOBAL_LATENTS + pal * pigment::PIGMENT_LATENTS;
            latents[base..base + pigment::PIGMENT_LATENTS].copy_from_slice(&live_pigment_latents);
        }
        let latents_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pigment_latents"),
            size: std::mem::size_of_val(&latents) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&latents_buffer, 0, bytemuck::cast_slice(&latents));

        // 顔料個性(M3): ρ/ω/γ。compute の binding 9 に渡す。M5 でランタイム編集可能(全レイヤー共通)
        let physics = palette.physics_uniform();
        let physics_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pigment_physics"),
            size: std::mem::size_of_val(&physics) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&physics_buffer, 0, bytemuck::cast_slice(&physics));

        // パン/ズーム(M6): display 専用のビューポート変換 uniform。既定は全体表示(zoom=1)。
        // フレームごとに CanvasCallback が現在のビュー状態を書き込む
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view_transform"),
            size: std::mem::size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&view_buffer, 0, bytemuck::bytes_of(&ViewUniform::default()));

        // アクティブタイル(M6): タイルごとの計算有効フラグ(u32 × NUM_TILES)。
        // raw = tilescan の生フラグ、active = tiledilate で1タイル膨張した最終フラグ。
        // シミュ各パス(binding 11)と display(binding 12)が active を読む。
        // 毎フレーム tilescan が全要素を書き直すので初期値は問わない(zero 初期化のまま)
        let tile_flags_size = (super::NUM_TILES as usize * std::mem::size_of::<u32>()) as u64;
        let raw_active_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile_raw_active"),
            size: tile_flags_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let active_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile_active"),
            size: tile_flags_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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
        // 8 = 紙ハイト(M1d、静的), 9 = 顔料個性 ρ/ω/γ(M3、静的),
        // 10 = 清書ペンの線画(M4.5b、透水率の境界。velocity/diffuse だけが読む。他パスは宣言せず素通し)
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
                buffer_entry(
                    9,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Uniform,
                ),
                sampled_entry(10, wgpu::ShaderStages::COMPUTE),
                // アクティブタイル(M6): タイル有効フラグ。各シミュパスが読み、非アクティブを素通しする
                buffer_entry(
                    11,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
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
                // 線画(M4.5a): 鉛筆(8)・ペン(9)の r32float。合成で色の上に重ねる
                sampled_entry(8, wgpu::ShaderStages::FRAGMENT),
                sampled_entry(9, wgpu::ShaderStages::FRAGMENT),
                // ハイライト(M4.5c): 不透明白。合成の最後に mix(色, 白, ハイライト)
                sampled_entry(10, wgpu::ShaderStages::FRAGMENT),
                // ビューポート変換(M6): パン/ズーム。fs_main が画面 uv → キャンバス uv に使う
                buffer_entry(
                    11,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BufferBindingType::Uniform,
                ),
                // アクティブタイル(M6): 表示モード 7 の可視化でタイル有効フラグを読む
                buffer_entry(
                    12,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BufferBindingType::Storage { read_only: true },
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

        // ラスタ線画(M4.5a): 対象の線画テクスチャ(read_write)+ params + splats + 紙ハイト。
        // 描画先(鉛筆 / ペン)は bind group を差し替えて選ぶ(パイプラインは1本)
        let raster_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("raster_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::ReadWrite,
                        format: wgpu::TextureFormat::R32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                buffer_entry(
                    1,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Uniform,
                ),
                buffer_entry(
                    2,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                sampled_entry(3, wgpu::ShaderStages::COMPUTE),
            ],
        });

        // アクティブタイル走査(M6、tilescan): 水/浮遊/沈着(read)+ params + splats +
        // raw_active(write)。current テクスチャを src として読むので bind group は parity 別
        let tilescan_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tilescan_bgl"),
            entries: &[
                sampled_entry(0, wgpu::ShaderStages::COMPUTE),
                sampled_entry(1, wgpu::ShaderStages::COMPUTE),
                sampled_entry(2, wgpu::ShaderStages::COMPUTE),
                buffer_entry(
                    3,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Uniform,
                ),
                buffer_entry(
                    4,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                buffer_entry(
                    5,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: false },
                ),
            ],
        });

        // アクティブタイル膨張(M6、tiledilate): raw_active(read)→ active(write)
        let tiledilate_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tiledilate_bgl"),
            entries: &[
                buffer_entry(
                    0,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                ),
                buffer_entry(
                    1,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: false },
                ),
                // common.wgsl の pressure_curve が params を参照するため束ねる(このパスでは未使用)
                buffer_entry(
                    2,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Uniform,
                ),
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
            entries.push(wgpu::BindGroupEntry {
                binding: 9,
                resource: physics_buffer.as_entire_binding(),
            });
            // 清書ペンの線画(M4.5b): velocity/diffuse が透水率の境界として読む。
            // ping-pong しないので src/dst に依らず固定([1] = ペン)
            entries.push(wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&line_views[1]),
            });
            // アクティブタイル(M6): タイル有効フラグ。src/dst に依らず固定
            entries.push(wgpu::BindGroupEntry {
                binding: 11,
                resource: active_buffer.as_entire_binding(),
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
                resource: latents_buffer.as_entire_binding(),
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
            entries.push(wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&line_views[0]),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(&line_views[1]),
            });
            // ハイライト(M4.5c): 合成の最後に白を重ねる
            entries.push(wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&line_views[2]),
            });
            // ビューポート変換(M6): パン/ズーム
            entries.push(wgpu::BindGroupEntry {
                binding: 11,
                resource: view_buffer.as_entire_binding(),
            });
            // アクティブタイル(M6): 表示モード 7 の可視化
            entries.push(wgpu::BindGroupEntry {
                binding: 12,
                resource: active_buffer.as_entire_binding(),
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("display_bg"),
                layout: &display_bgl,
                entries: &entries,
            })
        };
        let display_bind_groups = [make_display_bg(0), make_display_bg(1)];

        // ラスタ線画(M4.5a): 描画先(鉛筆 / ペン)ごとの bind group。線画は ping-pong しない
        // ので current に依存せず固定。paper_view は鉛筆の粒状変調に使う
        let make_raster_bg = |i: usize, label: &str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &raster_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&line_views[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: splat_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&paper_view),
                    },
                ],
            })
        };
        let raster_bind_groups = [
            make_raster_bg(0, "raster_pencil_bg"),
            make_raster_bg(1, "raster_pen_bg"),
            make_raster_bg(2, "raster_highlight_bg"),
        ];

        // アクティブタイル走査(M6): current テクスチャ(parity 別)を読んで raw_active を書く。
        // 水[0]/浮遊[1]/沈着[2] の各 parity ビューを 0/1/2 に、params/splats/raw を 3/4/5 に
        let make_tilescan_bg = |i: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("tilescan_bg"),
                layout: &tilescan_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&sim_views[0][i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&sim_views[1][i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&sim_views[2][i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: splat_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: raw_active_buffer.as_entire_binding(),
                    },
                ],
            })
        };
        let tilescan_bind_groups = [make_tilescan_bg(0), make_tilescan_bg(1)];

        // アクティブタイル膨張(M6): raw_active → active(バッファ固定なので1つ)
        let tiledilate_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tiledilate_bg"),
            layout: &tiledilate_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: raw_active_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: active_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

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
        let raster_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("raster_pipeline_layout"),
            bind_group_layouts: &[Some(&raster_bgl)],
            immediate_size: 0,
        });
        let tilescan_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tilescan_pipeline_layout"),
            bind_group_layouts: &[Some(&tilescan_bgl)],
            immediate_size: 0,
        });
        let tiledilate_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tiledilate_pipeline_layout"),
            bind_group_layouts: &[Some(&tiledilate_bgl)],
            immediate_size: 0,
        });

        let mut canvas = Self {
            shader_dir,
            target_format,
            textures,
            sim_views,
            wet_backup,
            paper_view,
            compute_bind_groups,
            display_bind_groups,
            params_buffer,
            splat_buffer,
            latents_buffer,
            physics_buffer,
            view_buffer,
            live_pigment_latents,
            compute_layout,
            display_layout,
            dried_slice_views,
            layers_buffer,
            bake_bgl,
            bake_layout,
            line_textures,
            raster_bind_groups,
            raster_layout,
            tilescan_bind_groups,
            tiledilate_bind_group,
            tilescan_layout,
            tiledilate_layout,
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
}
