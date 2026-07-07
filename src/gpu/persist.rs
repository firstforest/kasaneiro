//! 作品保存(M7)のための GPU 読み戻し / 書き戻し。gpu/mod.rs から分離。
//!
//! 描きかけの状態(湿レイヤー含む)を1ファイルへ保存/復元するために、GPU 上の全テクスチャと
//! レイヤーごとパレット(M5c)の latent バッファを CPU 側 f32 配列へ読み戻す([`WorkTextures`])。
//! ファイル入出力・シリアライズは GPU 非依存の [`crate::work`] が受け持ち、ここは
//! 「GPU ⇄ f32 配列」の変換だけに徹する(H6 の snapshot と同じく手動操作なので同期待ちで良い)。
//!
//! ping-pong の parity は保存しない。復元時は常に parity 0 側へ書き込み `current = 0` に正規化する
//! (次フレームの sim が 0 を読んで 1 へ書くので ping-pong の一貫性は保たれる)。

use super::{GpuCanvas, LATENT_TOTAL, TEX_KINDS};
use eframe::egui_wgpu::wgpu;
use paint_core::sim::CANVAS_SIZE;

/// エクスポート時に読み戻した全テクスチャ / latent の生データ(f32)。GPU 非依存の中間表現。
/// 各ブロブは行連続(パディングなし)。CANVAS_SIZE² を単位に並ぶ。
#[derive(Clone, PartialEq)]
pub struct WorkTextures {
    /// [水, 浮遊, 沈着] 各 CANVAS²·4 f32(rgba32float、現在表示中の parity)
    pub wet: [Vec<f32>; TEX_KINDS],
    /// 乾燥レイヤー(使用スロットのみ、slot 昇順)。各 CANVAS²·4 f32
    pub dried: Vec<Vec<f32>>,
    /// [鉛筆, ペン, ハイライト] 各 CANVAS²·1 f32(r32float)
    pub lines: [Vec<f32>; 3],
    /// 顔料 + 光学 latent(M1c/M5c)。LATENT_TOTAL·4 f32
    pub latents: Vec<f32>,
}

/// rgba32float(4成分)テクスチャ1テクセルあたりの f32 数
const RGBA: u32 = 4;

impl GpuCanvas {
    /// 現在のキャンバス状態を [`WorkTextures`] へ読み戻す(M7 保存)。
    /// 手動操作なので各テクスチャを同期で読む(H6 snapshot と同じ割り切り)。
    pub fn export_state(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<WorkTextures, String> {
        let cur = self.current;
        let wet = [
            self.read_texture(device, queue, &self.textures[0][cur], 0, RGBA)?,
            self.read_texture(device, queue, &self.textures[1][cur], 0, RGBA)?,
            self.read_texture(device, queue, &self.textures[2][cur], 0, RGBA)?,
        ];
        // 乾燥レイヤーはスロット 0..len を焼き込み順(= slot 昇順)に読む。
        // bake は slot = layers.len() で採番し解放しないので使用スロットは常に連続 0..len
        let mut dried = Vec::with_capacity(self.layers.len());
        for slot in 0..self.layers.len() as u32 {
            dried.push(self.read_texture(device, queue, &self.dried_texture, slot, RGBA)?);
        }
        let lines = [
            self.read_texture(device, queue, &self.line_textures[0], 0, 1)?,
            self.read_texture(device, queue, &self.line_textures[1], 0, 1)?,
            self.read_texture(device, queue, &self.line_textures[2], 0, 1)?,
        ];
        let latents = self.read_buffer(
            device,
            queue,
            &self.latents_buffer,
            (LATENT_TOTAL * 4 * 4) as u64,
        )?;
        Ok(WorkTextures {
            wet,
            dried,
            lines,
            latents,
        })
    }

    /// [`WorkTextures`] をキャンバスへ書き戻す(M7 読込)。全テクスチャを parity 0 側へ書き、
    /// `current = 0` に正規化する。レイヤー構成 / パレットは呼び出し側(app)が別途復元する。
    pub fn import_state(
        &mut self,
        queue: &wgpu::Queue,
        work: &WorkTextures,
    ) -> Result<(), String> {
        let texels = (CANVAS_SIZE * CANVAS_SIZE) as usize;
        // 湿レイヤー(rgba32float)を parity 0 へ
        for kind in 0..TEX_KINDS {
            if work.wet[kind].len() != texels * RGBA as usize {
                return Err("作品データの湿レイヤーのサイズが不正です".to_owned());
            }
            self.write_texture(queue, &self.textures[kind][0], 0, RGBA, &work.wet[kind]);
        }
        self.current = 0;

        // 乾燥レイヤー: slot 昇順に書き戻す(export と同順)
        for (slot, data) in work.dried.iter().enumerate() {
            if data.len() != texels * RGBA as usize {
                return Err("作品データの乾燥レイヤーのサイズが不正です".to_owned());
            }
            self.write_texture(queue, &self.dried_texture, slot as u32, RGBA, data);
        }

        // 線画(r32float)
        for i in 0..3 {
            if work.lines[i].len() != texels {
                return Err("作品データの線画のサイズが不正です".to_owned());
            }
            self.write_texture(queue, &self.line_textures[i], 0, 1, &work.lines[i]);
        }

        // 顔料 + 光学 latent(グローバル + レイヤーごとパレット + live)を丸ごと復元
        if work.latents.len() != LATENT_TOTAL * 4 {
            return Err("作品データのパレット latent のサイズが不正です".to_owned());
        }
        queue.write_buffer(&self.latents_buffer, 0, bytemuck::cast_slice(&work.latents));
        Ok(())
    }

    /// 2D テクスチャ(または array の1スライス)を f32 配列へ読み戻す。
    /// `components` = テクセルあたりの f32 数(rgba32float=4 / r32float=1)。
    /// CANVAS_SIZE=512 では行バイト数が 256 の倍数なのでパディングは生じない
    fn read_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        base_layer: u32,
        components: u32,
    ) -> Result<Vec<f32>, String> {
        let bytes_per_row = CANVAS_SIZE * components * 4;
        let size = (bytes_per_row * CANVAS_SIZE) as u64;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("work_texture_readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("work_texture_readback_encoder"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: base_layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
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
        self.map_read_f32(device, &staging)
    }

    /// バッファ(latent uniform 等)を f32 配列へ読み戻す
    fn read_buffer(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
        size: u64,
    ) -> Result<Vec<f32>, String> {
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("work_buffer_readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("work_buffer_readback_encoder"),
        });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        queue.submit([encoder.finish()]);
        self.map_read_f32(device, &staging)
    }

    /// staging バッファを map して f32 の Vec に読み出す(GPU 完了を同期待ち)。
    /// snapshot() と同じ待ち方(手動操作なので 1 フレームの停止は許容)
    fn map_read_f32(
        &self,
        device: &wgpu::Device,
        staging: &wgpu::Buffer,
    ) -> Result<Vec<f32>, String> {
        let slice = staging.slice(..);
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
        let data = slice.get_mapped_range();
        let floats = bytemuck::cast_slice::<u8, f32>(&data).to_vec();
        drop(data);
        staging.unmap();
        Ok(floats)
    }

    /// f32 配列を 2D テクスチャ(または array の1スライス)へ書き込む。read_texture の逆
    fn write_texture(
        &self,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        base_layer: u32,
        components: u32,
        data: &[f32],
    ) {
        let bytes_per_row = CANVAS_SIZE * components * 4;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: base_layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
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
