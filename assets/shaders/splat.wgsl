// splat.wgsl — ブラシで「水+顔料を置く」compute パス(M1a: 水量+初速 / M1b: 顔料)。
// 先頭に common.wgsl が連結される(SimParams / Splat / ヘルパー関数はそちら)。

@group(0) @binding(0) var src_water: texture_2d<f32>;
@group(0) @binding(1) var dst_water: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var src_susp: texture_2d<f32>;
@group(0) @binding(3) var dst_susp: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var src_dep: texture_2d<f32>;
@group(0) @binding(5) var dst_dep: texture_storage_2d<rgba32float, write>;
@group(0) @binding(6) var<uniform> params: SimParams;
@group(0) @binding(7) var<storage, read> splat_buf: SplatBuffer;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(src_water);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }

    let p = vec2f(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let ip = vec2i(gid.xy);
    let cell = textureLoad(src_water, ip, 0);
    var susp = textureLoad(src_susp, ip, 0);
    var water = cell.r;
    var vel = cell.gb;
    var wet = cell.a;

    for (var i = 0u; i < splat_buf.count; i++) {
        let s = splat_buf.splats[i];
        // 筆圧マッピング(M1.5): 半径・水量・顔料量それぞれに「効き」スライダーで反映。
        // CPU 側の SimParams::radius_at と同じ式(サンプル間隔の算出に使う)
        let press = pressure_curve(s.pressure);
        let radius = max(params.brush_radius * mix(1.0, press, params.pressure_radius), 0.5);
        let dist = distance(p, s.pos);
        // 中心は満量、縁にかけて柔らかく減衰
        let coverage = 1.0 - smoothstep(radius * 0.6, radius, dist);
        water += coverage * params.brush_water * mix(1.0, press, params.pressure_water);
        vel += coverage * params.brush_velocity * s.vel;
        // 顔料は浮遊層の選択チャンネル(brush_channel = パレットの顔料スロット)へ注入する
        susp[min(params.brush_channel, 3u)] += coverage * params.brush_pigment * mix(1.0, press, params.pressure_pigment);
        // 筆が届いた範囲を濡らす(wet-area mask)。水が動けるのはこの領域だけ。
        // 水を置く範囲(coverage > 0)と一致させ、マスク外に水が取り残されないようにする
        if (dist < radius) {
            wet = 1.0;
        }
    }

    // 安定性: 水量は非負、速度は CFL 的上限でクランプ
    water = max(water, 0.0);
    let speed = length(vel);
    if (speed > params.vel_max) {
        vel *= params.vel_max / speed;
    }

    textureStore(dst_water, ip, vec4f(water, vel, wet));
    textureStore(dst_susp, ip, susp);
    // 沈着顔料は変更なし(素通し)
    textureStore(dst_dep, ip, textureLoad(src_dep, ip, 0));
}
