//! フレーム描画コールバック(egui-wgpu)。
//!
//! `CanvasCallback` は src/app/ui/canvas.rs で組み立てて egui の PaintCallback として渡され、
//! prepare で compute パス(splat → シミュレーション本体)を積み、paint で表示する。
//! compute を積む本体は [`GpuCanvas::encode_frame`] で、headless 駆動(e2e)も同じ経路を通る。
//! パス実行順(encode_frame 内)はシミュレーションの心臓部で、ここがその正典。

use super::{GpuCanvas, LineTarget, MAX_DIFFUSE_ITERS, MAX_RELAX_ITERS, ViewUniform};
use eframe::egui_wgpu::{self, wgpu};
use paint_core::load::BrushFrame;
use paint_core::sim::{MAX_SPLATS, SimParams, Splat, SplatHeader};
use paint_core::tool::WetTool;
use std::sync::atomic::Ordering;

/// 1フレーム分の描画データ。src/app/ui/canvas.rs で組み立てて PaintCallback として渡す。
pub struct CanvasCallback {
    pub params: SimParams,
    pub splats: Vec<Splat>,
    /// このフレームで進めるシミュレーションステップ数(H6: 0=一時停止中)
    pub sim_steps: u32,
    /// ラスタ線画ツール(M4.5a)選択中の描画先。Some のときは splat を流体でなく
    /// 対応する線画テクスチャへ linesplat.wgsl で直描きする(水は注入しない)
    pub line_target: Option<LineTarget>,
    /// パン/ズーム(M6): display の画面 uv → キャンバス uv 変換。毎フレーム反映する
    pub view: ViewUniform,
    /// 塗る筆の中身への指示(置き換え・なぞった減り・置いたままの含み)
    pub brush: BrushFrame,
}

impl egui_wgpu::CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(canvas) = callback_resources.get_mut::<GpuCanvas>() {
            // 前のフレームの筆の中身の読み戻し(そのコピーは submit 済み)
            canvas.poll_brush(device);
            canvas.encode_frame(queue, egui_encoder, self);
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

impl GpuCanvas {
    /// 1フレーム分(splat 注入 → シミュレーション `frame.sim_steps` ステップ)の compute パスを
    /// `encoder` に積む。egui の prepare と headless 駆動([`crate::headless`])の共通経路で、
    /// submit は呼び出し側が行う
    pub fn encode_frame(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &CanvasCallback,
    ) {
        let canvas = self;
        queue.write_buffer(&canvas.params_buffer, 0, bytemuck::bytes_of(&frame.params));
        // M6: パン/ズームの変換を display 用 uniform へ(compute には影響しない)
        queue.write_buffer(&canvas.view_buffer, 0, bytemuck::bytes_of(&frame.view));
        // 筆の中身の置き換えはこのフレームの splat より前に効く
        if let Some(load) = frame.brush.set {
            canvas.brush.set(queue, load);
        }

        if frame.splats.len() > MAX_SPLATS {
            log::warn!(
                "1フレームの splat が上限を超えたため {} 個を切り捨てます({} > {MAX_SPLATS})",
                frame.splats.len() - MAX_SPLATS,
                frame.splats.len()
            );
        }
        let splat_count = frame.splats.len().min(MAX_SPLATS);
        // ヘッダは splat 0 個のフレームでも毎回書く(tilescan が前フレームの count・位置で
        // タイルを立て続けないように)。線画ツールのフレームは流体側に splat を見せない
        let header = SplatHeader {
            count: splat_count as u32,
            fluid_count: if frame.line_target.is_none() {
                splat_count as u32
            } else {
                0
            },
            brush_scale: frame.brush.scale,
            feed_dt: frame.brush.feed_dt,
        };
        let mut bytes = Vec::with_capacity(
            std::mem::size_of::<SplatHeader>() + splat_count * std::mem::size_of::<Splat>(),
        );
        bytes.extend_from_slice(bytemuck::bytes_of(&header));
        bytes.extend_from_slice(bytemuck::cast_slice(&frame.splats[..splat_count]));
        queue.write_buffer(&canvas.splat_buffer, 0, &bytes);

        // 乾燥レイヤーの合成キャッシュ: レイヤー側が変わったか、キャッシュに効く SimParams が
        // 変わったフレームだけ焼き直す(display はこのフレームの paint でキャッシュを読む)
        let cache_key = (
            frame.params.pigment_density.to_bits(),
            frame.params.compose_mode,
        );
        if let Some(pipelines) = &canvas.pipelines
            && (canvas.dried_cache_dirty.load(Ordering::Relaxed)
                || canvas.dried_cache_key != Some(cache_key))
        {
            canvas.encode_dried_cache(encoder, pipelines);
            canvas.dried_cache_dirty.store(false, Ordering::Relaxed);
            canvas.dried_cache_key = Some(cache_key);
        }

        let mut current = canvas.current;
        let mut brush_ran = false;
        if let Some(pipelines) = &canvas.pipelines {
            let workgroups = canvas.size.div_ceil(8);

            // ラスタ線画(M4.5a/c): 対象の線画テクスチャ(read_write)へ直描きする。
            // 流体パスが清書ペンの線を sampled で読む(M4.5b の透水率)ため、同一 compute パスに
            // 混ぜると同じテクスチャが read_write と sampled を兼ねて使用範囲が衝突する。別パスに分ける。
            // ブラシ入力は一時停止中でも反映する(ping-pong しないので current は反転しない)。
            if splat_count > 0
                && let Some(target) = frame.line_target
            {
                let mut line_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("line_pass"),
                    timestamp_writes: None,
                });
                line_pass.set_pipeline(pipelines.compute("linesplat.wgsl"));
                line_pass.set_bind_group(0, canvas.raster_bind_group(target), &[]);
                line_pass.dispatch_workgroups(workgroups, workgroups, 1);
            }

            let bind_groups = &canvas.compute_bind_groups;
            let run_splat = splat_count > 0 && frame.line_target.is_none();
            // ping-pong するパスが1つもないフレーム(一時停止中で筆も流体へ流さない)は
            // タイル判定ごと積まない。tiledilate の dirty 更新は「このフレームに少なくとも1パス進む」
            // ことを前提にしているため(tiledilate.wgsl)。active は前フレームの値が残る
            if !run_splat && frame.sim_steps == 0 {
                canvas.brush.encode_readback(encoder);
                return;
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sim_pass"),
                timestamp_writes: None,
            });

            // アクティブタイル(M6): シミュ本体より前にタイルの状態を作る。
            // tilescan(濡れ+ブラシ→raw_active)→ tiledilate(1タイル膨張+dirty→active)。
            // 以降の splat / sim 各パスは active を読み、ACTIVE だけ計算・COPY はコピー・COLD は何もしない。
            // active_tiles=0 のときは tilescan が全タイルを有効化するので全面計算に戻る。
            // 同一パス内の storage RAW ハザードは wgpu が自動バリアで解決する(テクスチャ ping-pong と同様)
            let tile_workgroups = canvas.tiles_per_side.div_ceil(8);
            pass.set_pipeline(pipelines.compute("tilescan.wgsl"));
            pass.set_bind_group(0, &canvas.tilescan_bind_groups[current], &[]);
            pass.dispatch_workgroups(tile_workgroups, tile_workgroups, 1);
            pass.set_pipeline(pipelines.compute("tiledilate.wgsl"));
            pass.set_bind_group(0, &canvas.tiledilate_bind_group, &[]);
            pass.dispatch_workgroups(tile_workgroups, tile_workgroups, 1);

            // 1 dispatch = current を読み、もう片方へ書き、反転(ping-pong)
            let mut run = |pass: &mut wgpu::ComputePass, pipeline: &wgpu::ComputePipeline| {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind_groups[current], &[]);
                pass.dispatch_workgroups(workgroups, workgroups, 1);
                current ^= 1;
            };

            // 流体ツールのときだけ splat を流す(ラスタツールは上の line_pass で処理済み)
            if run_splat {
                run(&mut pass, pipelines.compute("splat.wgsl"));
                // 塗る筆: なぞった減り・含み・吸い込みを筆の中身へ反映する。テクスチャは読み書き
                // しない(ping-pong しない)ので、筆のバッファが同じならどちらの bind group でもよい
                if frame.params.tool == WetTool::Paint.gpu_id() {
                    pass.set_pipeline(pipelines.compute("brush.wgsl"));
                    pass.set_bind_group(0, &bind_groups[0], &[]);
                    pass.dispatch_workgroups(1, 1, 1);
                    brush_ran = true;
                }
            }
            // 1 ステップ = 速度更新 → 発散緩和 × N → FlowOutward → 移流
            //   → 顔料拡散 × N → 吸着/脱着+蒸発(パス実行順はここがハードコードの正典)
            let relax_iters = frame.params.relax_iters.clamp(1, MAX_RELAX_ITERS);
            let diffuse_iters = frame.params.diffuse_iters.min(MAX_DIFFUSE_ITERS);
            for _ in 0..frame.sim_steps {
                run(&mut pass, pipelines.compute("velocity.wgsl"));
                for _ in 0..relax_iters {
                    run(&mut pass, pipelines.compute("relax.wgsl"));
                }
                // エッジダークニング(M1d)。η=0 ならぼかしの読み出しごと省略
                if frame.params.edge_eta > 0.0 {
                    run(&mut pass, pipelines.compute("flowout.wgsl"));
                }
                run(&mut pass, pipelines.compute("advect.wgsl"));
                for _ in 0..diffuse_iters {
                    run(&mut pass, pipelines.compute("diffuse.wgsl"));
                }
                run(&mut pass, pipelines.compute("transfer.wgsl"));
            }
        }
        canvas.current = current;
        if brush_ran {
            canvas.brush.touched();
        }
        canvas.brush.encode_readback(encoder);
    }
}
