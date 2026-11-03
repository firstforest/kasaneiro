//! PNG スナップショット(H6)の読み戻し。
//!
//! display と同じシェーダーをスナップショット用フォーマットでオフスクリーンに焼き、
//! テクスチャ → バッファ → map で RGBA8 の Vec に読み戻す。スポイト用に湿レイヤーの顔料を
//! 小さな窓だけ読み戻す [`GpuCanvas::read_wet_pigment`] もここに置く。作品保存の読み戻しは persist.rs に別にある。

use super::{GpuCanvas, ViewUniform};
use eframe::egui_wgpu::wgpu;
use std::sync::atomic::Ordering;

impl GpuCanvas {
    /// 現在のキャンバス表示を RGBA8(行連続、size²)で読み戻す(H6 PNG スナップショット)。
    /// display と同じシェーダー・同じ表示モードで、ビュー変換 `view` を当てて焼く
    /// (PNG 書き出しは `ViewUniform::default()` = 全体・無回転、スポイトは画面と同じビュー)。
    /// view_buffer は表示と共有で、次フレームの encode_frame が表示用に書き直す。
    /// GPU 完了を同期で待つ(手動ボタン操作なので 1 フレームの停止は許容)。
    pub fn snapshot(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &ViewUniform,
    ) -> Result<Vec<u8>, String> {
        let pipelines = self.pipelines.as_ref().ok_or("シェーダーが未ビルドです")?;

        queue.write_buffer(&self.view_buffer, 0, bytemuck::bytes_of(view));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("snapshot_encoder"),
        });
        self.encode_offscreen_display(&mut encoder, pipelines);
        encoder.copy_texture_to_buffer(
            self.snapshot_texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.snapshot_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    // CANVAS_SIZES は 64 の倍数なので 256B 整列を満たす(M8)
                    bytes_per_row: Some(self.size * 4),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: self.size,
                height: self.size,
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

    /// スポイト: キャンバステクセル (x, y) を中心とする (2·radius+1)² の窓で、湿レイヤーの
    /// 浮遊 + 沈着顔料(4スロット)の1テクセルあたり平均を読み戻す。窓はキャンバス内に切り詰める。
    /// GPU 完了を同期で待つ(クリック1回の操作なので 1 フレームの停止は許容)
    pub fn read_wet_pigment(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        x: u32,
        y: u32,
        radius: u32,
    ) -> Result<[f32; 4], String> {
        const TEXEL: u32 = 16; // rgba32float
        let x0 = x.min(self.size - 1).saturating_sub(radius);
        let y0 = y.min(self.size - 1).saturating_sub(radius);
        let w = (x.min(self.size - 1) + radius + 1).min(self.size) - x0;
        let h = (y.min(self.size - 1) + radius + 1).min(self.size) - y0;
        let row = (w * TEXEL).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let kind_bytes = (row * h) as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("eyedropper_readback"),
            size: kind_bytes * 2,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("eyedropper_encoder"),
        });
        // [水, 浮遊, 沈着] のうち浮遊(1)と沈着(2)
        for (i, kind) in [1usize, 2].into_iter().enumerate() {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[kind][self.current],
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: kind_bytes * i as u64,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
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
            .map_err(|e| format!("スポイトの読み戻しに失敗: {e:?}"))?;
        let data = slice.get_mapped_range();
        let floats: &[f32] = bytemuck::cast_slice(&data);
        let mut sum = [0.0f32; 4];
        let row_floats = (row / 4) as usize;
        for k in 0..2 {
            let base = (kind_bytes / 4) as usize * k;
            for yy in 0..h as usize {
                for xx in 0..w as usize {
                    let at = base + yy * row_floats + xx * 4;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += floats[at + c];
                    }
                }
            }
        }
        drop(data);
        buffer.unmap();
        let n = (w * h) as f32;
        Ok(sum.map(|s| s / n))
    }

    /// display と同じシェーダーでオフスクリーン(snapshot_texture)に1回描く。読み戻しはしない。
    /// 表示コストの計測(tests/perf_bench.rs)用で、完了は待たない
    pub fn render_offscreen(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(), String> {
        let pipelines = self.pipelines.as_ref().ok_or("シェーダーが未ビルドです")?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offscreen_encoder"),
        });
        self.encode_offscreen_display(&mut encoder, pipelines);
        queue.submit([encoder.finish()]);
        Ok(())
    }

    /// オフスクリーン描画を積む。フレームを挟まずに呼ばれる(乾かした直後のスナップショットなど)ので、
    /// 合成キャッシュが古ければ先に焼く。dirty は下ろさず、次の encode_frame がそのフレームの
    /// SimParams で焼き直す(params_buffer は rasterize_line が別の値で上書きしていることがある)
    fn encode_offscreen_display(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &super::Pipelines,
    ) {
        if self.dried_cache_dirty.load(Ordering::Relaxed) {
            self.encode_dried_cache(encoder, pipelines);
        }
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
}
