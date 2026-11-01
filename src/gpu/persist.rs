//! 作品保存(M7)のための GPU 読み戻し / 書き戻し。
//!
//! 描きかけの状態(湿レイヤー含む)を1ファイルへ保存/復元するために、GPU 上の全テクスチャと
//! レイヤーごとパレット(M5c)の latent バッファを CPU 側 f32 配列へ読み戻す([`WorkTextures`])。
//! ファイル入出力・シリアライズは GPU 非依存の [`crate::work`] が受け持ち、ここは
//! 「GPU ⇄ f32 配列」の変換だけに徹する(H6 の snapshot と同じく手動操作なので同期待ちで良い)。
//!
//! ping-pong の parity は保存しない。復元時は常に parity 0 側へ書き込み `current = 0` に正規化する
//! (次フレームの sim が 0 を読んで 1 へ書くので ping-pong の一貫性は保たれる)。

use super::{GpuCanvas, LATENT_TOTAL, MAX_LAYERS, TEX_KINDS};
use eframe::egui_wgpu::wgpu;
use std::sync::atomic::Ordering;

/// エクスポート時に読み戻した全テクスチャ / latent の生データ(f32)。GPU 非依存の中間表現。
/// 各ブロブは行連続(パディングなし)。キャンバス1辺²(M8 で可変)を単位に並ぶ。
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
    /// 全テクスチャ・latent のコピーを1本の encoder に積んで1回 submit し、全 staging を
    /// map してから1回だけ GPU 完了を待つ(手動操作なので同期待ちで良い。H6 snapshot と同じ割り切り)
    pub fn export_state(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<WorkTextures, String> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("work_readback_encoder"),
        });
        let cur = self.current;
        let wet: Vec<wgpu::Buffer> = (0..TEX_KINDS)
            .map(|kind| self.copy_texture(device, &mut encoder, &self.textures[kind][cur], 0, RGBA))
            .collect();
        // 乾燥レイヤーはスロット 0..len を焼き込み順(= slot 昇順)に読む。
        // bake は slot = layers.len() で採番し解放しないので使用スロットは常に連続 0..len
        let dried: Vec<wgpu::Buffer> = (0..self.layers.len() as u32)
            .map(|slot| self.copy_texture(device, &mut encoder, &self.dried_texture, slot, RGBA))
            .collect();
        let lines: Vec<wgpu::Buffer> = self
            .line_textures
            .iter()
            .map(|t| self.copy_texture(device, &mut encoder, t, 0, 1))
            .collect();
        let latents = copy_buffer(
            device,
            &mut encoder,
            &self.latents_buffer,
            (LATENT_TOTAL * 4 * 4) as u64,
        );
        queue.submit([encoder.finish()]);

        let mut all: Vec<&wgpu::Buffer> = wet.iter().chain(&dried).chain(&lines).collect();
        all.push(&latents);
        let mut floats = map_read_all(device, &all)?.into_iter();
        let mut take = |n: usize| -> Vec<Vec<f32>> { floats.by_ref().take(n).collect() };
        let wet: [Vec<f32>; TEX_KINDS] = take(TEX_KINDS).try_into().expect("湿レイヤー3枚");
        let dried = take(dried.len());
        let lines: [Vec<f32>; 3] = take(3).try_into().expect("線画3枚");
        let latents = take(1).pop().expect("latent");
        Ok(WorkTextures {
            wet,
            dried,
            lines,
            latents,
        })
    }

    /// ping-pong の2枚が食い違っているテクセル数を [水, 浮遊, 沈着] ごとに数える(検査用)。
    /// アクティブタイルが書き込みを省くタイルでは2枚が一致している前提なので、
    /// シミュが止まった(全部乾いた)状態では全種類 0 になるはず
    pub fn pingpong_mismatch(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<[usize; TEX_KINDS], String> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pingpong_readback_encoder"),
        });
        let stagings: Vec<wgpu::Buffer> = (0..TEX_KINDS)
            .flat_map(|kind| [0, 1].map(|parity| (kind, parity)))
            .map(|(kind, parity)| {
                self.copy_texture(device, &mut encoder, &self.textures[kind][parity], 0, RGBA)
            })
            .collect();
        queue.submit([encoder.finish()]);
        let all: Vec<&wgpu::Buffer> = stagings.iter().collect();
        let floats = map_read_all(device, &all)?;
        Ok(std::array::from_fn(|kind| {
            floats[kind * 2]
                .chunks(RGBA as usize)
                .zip(floats[kind * 2 + 1].chunks(RGBA as usize))
                .filter(|(a, b)| a != b)
                .count()
        }))
    }

    /// [`WorkTextures`] をキャンバスへ書き戻す(M7 読込)。全テクスチャを parity 0 側へ書き、
    /// `current = 0` に正規化する。レイヤー構成 / パレットは呼び出し側(app)が別途復元する。
    pub fn import_state(
        &mut self,
        queue: &wgpu::Queue,
        work: &WorkTextures,
    ) -> Result<(), String> {
        // 乾燥テクスチャのスライス数を超える書き込みは検証エラーになるので、書き始める前に弾く
        if work.dried.len() > MAX_LAYERS {
            return Err(format!(
                "作品データの乾燥レイヤーが多すぎます({} > {MAX_LAYERS})",
                work.dried.len()
            ));
        }
        let texels = (self.size * self.size) as usize;
        // 湿レイヤー(rgba32float)を parity 0 へ
        for kind in 0..TEX_KINDS {
            if work.wet[kind].len() != texels * RGBA as usize {
                return Err("作品データの湿レイヤーのサイズが不正です".to_owned());
            }
            self.write_texture(queue, &self.textures[kind][0], 0, RGBA, &work.wet[kind]);
        }
        self.current = 0;
        self.mark_tiles_dirty(queue);

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
        self.dried_cache_dirty.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// 2D テクスチャ(または array の1スライス)を staging バッファへコピーするコマンドを積む。
    /// `components` = テクセルあたりの f32 数(rgba32float=4 / r32float=1)。
    /// CANVAS_SIZES(64 の倍数)では行バイト数が 256 の倍数なのでパディングは生じない
    fn copy_texture(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        base_layer: u32,
        components: u32,
    ) -> wgpu::Buffer {
        let bytes_per_row = self.size * components * 4;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("work_texture_readback"),
            size: (bytes_per_row * self.size) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
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
                width: self.size,
                height: self.size,
                depth_or_array_layers: 1,
            },
        );
        staging
    }

    /// f32 配列を 2D テクスチャ(または array の1スライス)へ書き込む。copy_texture(読み戻し)の逆
    fn write_texture(
        &self,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        base_layer: u32,
        components: u32,
        data: &[f32],
    ) {
        let bytes_per_row = self.size * components * 4;
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
                width: self.size,
                height: self.size,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// バッファ(latent uniform 等)を staging バッファへコピーするコマンドを積む
fn copy_buffer(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    buffer: &wgpu::Buffer,
    size: u64,
) -> wgpu::Buffer {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("work_buffer_readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    staging
}

/// submit 済みのコピー先 staging バッファ群をまとめて map し、GPU 完了を1回だけ同期待ちしてから
/// 順に f32 の Vec へ読み出す(戻り値は `stagings` と同順)
fn map_read_all(device: &wgpu::Device, stagings: &[&wgpu::Buffer]) -> Result<Vec<Vec<f32>>, String> {
    let receivers: Vec<_> = stagings
        .iter()
        .map(|staging| {
            let (tx, rx) = std::sync::mpsc::channel();
            staging.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
            rx
        })
        .collect();
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .map_err(|e| format!("GPU の完了待ちに失敗: {e:?}"))?;
    let mut out = Vec::with_capacity(stagings.len());
    for (staging, rx) in stagings.iter().zip(receivers) {
        rx.recv()
            .map_err(|e| format!("map コールバックが届きません: {e}"))?
            .map_err(|e| format!("読み戻しバッファの map に失敗: {e:?}"))?;
        let data = staging.slice(..).get_mapped_range();
        out.push(bytemuck::cast_slice::<u8, f32>(&data).to_vec());
        drop(data);
        staging.unmap();
    }
    Ok(out)
}
