//! フレーム描画コールバック(egui-wgpu)。gpu/mod.rs から分離。
//!
//! `CanvasCallback` は app.rs で組み立てて egui の PaintCallback として渡され、
//! prepare で compute パス(splat → シミュレーション本体)を積み、paint で表示する。
//! パス実行順(prepare 内)はシミュレーションの心臓部で、ここがその正典(R3)。

use super::{GpuCanvas, LineTarget, MAX_DIFFUSE_ITERS, MAX_RELAX_ITERS, ViewUniform};
use paint_core::sim::{CANVAS_SIZE, MAX_SPLATS, SimParams, Splat, SplatHeader};
use eframe::egui_wgpu::{self, wgpu};

/// 1フレーム分の描画データ。app.rs で組み立てて PaintCallback として渡す。
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
        // M6: パン/ズームの変換を display 用 uniform へ(compute には影響しない)
        queue.write_buffer(&canvas.view_buffer, 0, bytemuck::bytes_of(&self.view));

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

            // ラスタ線画(M4.5a/c): 対象の線画テクスチャ(read_write)へ直描きする。
            // 流体パスが清書ペンの線を sampled で読む(M4.5b の透水率)ため、同一 compute パスに
            // 混ぜると同じテクスチャが read_write と sampled を兼ねて使用範囲が衝突する。別パスに分ける。
            // ブラシ入力は一時停止中でも反映する(ping-pong しないので current は反転しない)。
            if splat_count > 0
                && let Some(target) = self.line_target
            {
                let mut line_pass = egui_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("line_pass"),
                    timestamp_writes: None,
                });
                line_pass.set_pipeline(pipelines.compute("linesplat.wgsl"));
                line_pass.set_bind_group(0, canvas.raster_bind_group(target), &[]);
                line_pass.dispatch_workgroups(workgroups, workgroups, 1);
            }

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

            // 流体ツールのときだけ splat を流す(ラスタツールは上の line_pass で処理済み)
            if splat_count > 0 && self.line_target.is_none() {
                run(&mut pass, pipelines.compute("splat.wgsl"));
            }
            // 1 ステップ = 速度更新 → 発散緩和 × N → FlowOutward → 移流
            //   → 顔料拡散 × N → 吸着/脱着+蒸発(パス実行順はここがハードコードの正典。R3)
            let relax_iters = self.params.relax_iters.clamp(1, MAX_RELAX_ITERS);
            let diffuse_iters = self.params.diffuse_iters.min(MAX_DIFFUSE_ITERS);
            for _ in 0..self.sim_steps {
                run(&mut pass, pipelines.compute("velocity.wgsl"));
                for _ in 0..relax_iters {
                    run(&mut pass, pipelines.compute("relax.wgsl"));
                }
                // エッジダークニング(M1d)。η=0 ならぼかしの読み出しごと省略
                if self.params.edge_eta > 0.0 {
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
