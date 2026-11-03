//! 塗る筆の中身(GPU の筆バッファ)と、CPU から見るための読み戻し。
//!
//! 中身の正典は GPU の `state` バッファ。描いている間は splat.wgsl が読んで紙と水を受け渡し、
//! brush.wgsl が受け渡した量を毎フレーム中身へ反映する。CPU は
//! - 置き換え(色・水・洗う・自動補充・再生のストローク開始)を `set` で書き込み、
//! - 表示用に、中身が変わったフレームの値を非同期で読み戻して `mirror` に持つ(数フレーム遅れる)。
//!
//! 中身を CPU で編集する前(色を取る・水に浸すなど)と、記録に描き始めの中身を残すときは、
//! [`GpuBrush::sync`] で完了を待って最新の値を得る。描き終わってから次の操作までには
//! 普通は非同期の読み戻しが追いついているので、待つのは描いた直後に触ったときだけ。
//!
//! 混色皿([`super::MixCanvas`])は本体のキャンバスと同じ `state` バッファを使う(筆は1本)。
//! 読み戻しと CPU からの置き換えは本体の `GpuBrush` が受け持ち、混色皿の側は読み戻さない。

use eframe::egui_wgpu::wgpu;
use paint_core::load::{BrushLoad, BrushState};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// 読み戻しの map の結果(AtomicU8 に入れる値)
const MAP_PENDING: u8 = 0;
const MAP_OK: u8 = 1;
const MAP_FAILED: u8 = 2;

/// splat.wgsl / brush.wgsl の BrushFlow(atomic<i32> × 5)のバイト数
const FLOW_SIZE: u64 = 5 * 4;

pub(crate) struct GpuBrush {
    /// 筆の中身(binding 12)。COPY_SRC は読み戻し、COPY_DST は `set` のため
    pub(super) state: wgpu::Buffer,
    /// 筆と紙が受け渡した量の集計(binding 13)。brush.wgsl が毎回 0 に戻す
    pub(super) flow: wgpu::Buffer,
    /// 読み戻し用(MAP_READ)
    staging: wgpu::Buffer,
    /// GPU の中身の版。中身を書き換えうる操作(set・brush.wgsl)を積むたびに 1 進める
    version: u64,
    /// 最後に分かっている中身と、その版
    mirror: BrushLoad,
    mirror_version: u64,
    /// staging へのコピーを積んだ版(まだ map していない)
    copied: Option<u64>,
    /// map 中の版と結果
    mapping: Option<(u64, Arc<AtomicU8>)>,
    /// 中身を読み戻すか。別のキャンバスと `state` を共有している側(混色皿)は false
    readback: bool,
}

impl GpuBrush {
    /// `shared` = 別のキャンバスの筆の中身のバッファ(None なら作る)。`readback` = 中身を CPU へ読み戻すか
    pub(super) fn new(device: &wgpu::Device, shared: Option<wgpu::Buffer>, readback: bool) -> Self {
        let state = shared.unwrap_or_else(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("brush_state"),
                size: std::mem::size_of::<BrushState>() as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let flow = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush_flow"),
            size: FLOW_SIZE,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush_readback"),
            size: std::mem::size_of::<BrushState>() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // 作成直後のバッファはゼロ = 空の筆
        Self {
            state,
            flow,
            staging,
            version: 0,
            mirror: BrushLoad::empty(),
            mirror_version: 0,
            copied: None,
            mapping: None,
            readback,
        }
    }

    /// 中身を置き換える。次の submit の前に反映され、CPU 側の値もその場で確定する
    pub(super) fn set(&mut self, queue: &wgpu::Queue, load: BrushLoad) {
        queue.write_buffer(&self.state, 0, bytemuck::bytes_of(&BrushState::from(load)));
        self.version += 1;
        self.mirror = load;
        self.mirror_version = self.version;
    }

    /// brush.wgsl を積んだ(中身が変わりうる)
    pub(super) fn touched(&mut self) {
        self.version += 1;
    }

    /// 中身が読み戻した値より新しく、staging が空いていれば、コピーを積む。
    /// フレームの compute を積んだ最後に呼ぶ
    pub(super) fn encode_readback(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if self.readback
            && self.mirror_version < self.version
            && self.copied.is_none()
            && self.mapping.is_none()
        {
            encoder.copy_buffer_to_buffer(&self.state, 0, &self.staging, 0, self.staging.size());
            self.copied = Some(self.version);
        }
    }

    /// 前のフレームまでに積んだコピーの読み戻しを進める。コピーを積んだコマンドが
    /// submit 済みのときに呼ぶ(egui の prepare の頭 / headless の submit の後)。
    /// `wait` なら map が終わるまで待つ
    pub(super) fn poll(&mut self, device: &wgpu::Device, wait: bool) -> Result<(), String> {
        if let Some(version) = self.copied.take() {
            let flag = Arc::new(AtomicU8::new(MAP_PENDING));
            let done = flag.clone();
            self.staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let state = if result.is_ok() { MAP_OK } else { MAP_FAILED };
                    done.store(state, Ordering::Release);
                });
            self.mapping = Some((version, flag));
        }
        let Some((version, flag)) = self.mapping.clone() else {
            return Ok(());
        };
        let poll = if wait {
            wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            }
        } else {
            wgpu::PollType::Poll
        };
        device
            .poll(poll)
            .map_err(|e| format!("GPU の完了待ちに失敗: {e:?}"))?;
        match flag.load(Ordering::Acquire) {
            MAP_PENDING => Ok(()),
            MAP_OK => {
                let state: BrushState =
                    bytemuck::pod_read_unaligned(&self.staging.slice(..).get_mapped_range()[..]);
                self.staging.unmap();
                if version > self.mirror_version {
                    self.mirror = state.into();
                    self.mirror_version = version;
                }
                self.mapping = None;
                Ok(())
            }
            _ => {
                self.mapping = None;
                Err("筆の中身の読み戻しに失敗しました".to_owned())
            }
        }
    }

    /// 最後に分かっている中身(描いている間は数フレーム遅れる)
    pub(crate) fn mirror(&self) -> BrushLoad {
        self.mirror
    }

    /// GPU の最新の中身を待って得る。読み戻しが追いついていればそのまま返す
    pub(super) fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<BrushLoad, String> {
        // 投入済みの読み戻しを終わらせる
        while self.copied.is_some() || self.mapping.is_some() {
            self.poll(device, true)?;
        }
        if self.mirror_version < self.version {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("brush_sync"),
            });
            self.encode_readback(&mut encoder);
            queue.submit([encoder.finish()]);
            while self.copied.is_some() || self.mapping.is_some() {
                self.poll(device, true)?;
            }
        }
        Ok(self.mirror)
    }
}
