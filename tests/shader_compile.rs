//! assets/shaders/ の WGSL がコンパイル可能かの検査。
//! シミュレーションの挙動はテストしない方針(デバッグ表示 H4 で診断)だが、
//! 実行時ロードのため cargo build では捕まらない「壊れた WGSL のコミット」だけは防ぐ。
//! 連結ルール(common.wgsl を先頭に足す)は src/gpu/mod.rs の rebuild_pipelines() と同じ。

use std::path::Path;

#[test]
fn all_shaders_compile() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/shaders");
    let common = std::fs::read_to_string(dir.join("common.wgsl")).expect("common.wgsl");

    for name in [
        "splat.wgsl",
        "velocity.wgsl",
        "relax.wgsl",
        "flowout.wgsl",
        "advect.wgsl",
        "diffuse.wgsl",
        "transfer.wgsl",
        "bake.wgsl",
        "fastdry.wgsl",
        "rewet.wgsl",
        "linesplat.wgsl",
        "display.wgsl",
    ] {
        let src = std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let full = format!("{common}\n{src}");
        let module = naga::front::wgsl::parse_str(&full)
            .unwrap_or_else(|e| panic!("{name}: パースエラー:\n{}", e.emit_to_string(&full)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: 検証エラー: {e:?}"));
    }
}
