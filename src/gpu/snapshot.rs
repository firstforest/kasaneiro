//! PNG スナップショット(H6)の読み戻し。
//!
//! display と同じシェーダーをスナップショット用フォーマットでオフスクリーンに焼き、
//! テクスチャ → バッファ → map で RGBA8 の Vec に読み戻す。M5e(スポイト)・M7(全テクスチャ
//! の保存/復元)で汎用 readback が要るようになったら R8 でここを一般化する。

use super::{GpuCanvas, ViewUniform};
use eframe::egui_wgpu::wgpu;

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
        let pipelines = self
            .pipelines
            .as_ref()
            .ok_or("シェーダーが未ビルドです")?;

        queue.write_buffer(&self.view_buffer, 0, bytemuck::bytes_of(view));
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
}
