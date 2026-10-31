//! Rust と WGSL で共有する定数と、それを WGSL の const として生成するプレリュード。
//!
//! 生成したプレリュードは common.wgsl のさらに前に連結される(rebuild_pipelines)。
//! WGSL 側はここで定義した名前(TILE_SIZE・MAX_LAYERS・LATENT_TOTAL など)を参照し、
//! 値をベタ書きしない。tests/shader_compile.rs もこのファイルを `#[path]` で取り込み、
//! 同じプレリュードで全キャンバスサイズ分を検証する(二重管理しない)。
//! そのためこのファイルは crate 内の他モジュールに依存しない(外部 crate は pigment だけ)。

/// 乾燥レイヤー(M2)の上限枚数。texture array のスライス数として固定確保する
/// (512² rgba32float × 8 = 32MB)。display.wgsl の visible_mask が u32 なので 32 が理論上限、
/// LayerUniform.order を vec4u 単位で詰めるため 4 の倍数に限る
pub const MAX_LAYERS: usize = 8;

const _: () = assert!(MAX_LAYERS.is_multiple_of(4) && MAX_LAYERS <= 32);

/// latents uniform が持つパレット枠数(M5c)。乾燥レイヤー MAX_LAYERS 個 + 現行(live)1 個。
/// dried スロット s(0..MAX_LAYERS)は乾かした瞬間に現行パレットを焼き込んで固定し、
/// 現行(live)パレット枠 = 末尾(`LIVE_PALETTE`)は編集のたびに書き替える
pub const PALETTE_SLOTS: usize = MAX_LAYERS + 1;

/// 現行(live)パレットの枠番号(dried スロットの後ろ)。湿レイヤーの発色はここを使う
pub const LIVE_PALETTE: usize = MAX_LAYERS;

/// latents uniform の総 vec4 数 = グローバル光学 + パレット数 × 顔料ブロック
pub const LATENT_TOTAL: usize =
    pigment::GLOBAL_LATENTS + PALETTE_SLOTS * pigment::PIGMENT_LATENTS;

/// アクティブタイル(M6): 1タイルの1辺テクセル数
pub const TILE_SIZE: u32 = 16;

/// 各シェーダーの先頭(common.wgsl より前)に連結する WGSL の const 定義。
/// TILES_PER_SIDE はキャンバス1辺から決まる(M8)ので、ファイル(ホットリロード対象)でなく
/// Rust が生成する
pub fn shader_prelude(size: u32) -> String {
    format!(
        "const TILE_SIZE: u32 = {TILE_SIZE}u;\n\
         const TILES_PER_SIDE: u32 = {tiles}u;\n\
         const MAX_LAYERS: u32 = {MAX_LAYERS}u;\n\
         const LAYER_ORDER_VEC4S: u32 = {order}u;\n\
         const GLOBAL_LATENTS: u32 = {global}u;\n\
         const PIGMENT_LATENTS: u32 = {pigment}u;\n\
         const LIVE_PALETTE: u32 = {LIVE_PALETTE}u;\n\
         const LATENT_TOTAL: u32 = {LATENT_TOTAL}u;\n",
        tiles = size / TILE_SIZE,
        order = MAX_LAYERS / 4,
        global = pigment::GLOBAL_LATENTS,
        pigment = pigment::PIGMENT_LATENTS,
    )
}
