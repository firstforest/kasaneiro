//! assets/shaders/ の WGSL がコンパイル可能かの検査。
//! シミュレーションの挙動はテストしない方針(デバッグ表示 H4 で診断)だが、
//! 実行時ロードのため cargo build では捕まらない「壊れた WGSL のコミット」だけは防ぐ。
//! 連結ルール(プレリュード + common.wgsl を先頭に足す)は src/gpu/mod.rs の
//! rebuild_pipelines() と同じ。プレリュードは本体と同じ src/gpu/prelude.rs を取り込んで生成し、
//! キャンバスサイズ依存の定数があるので全サイズ(CANVAS_SIZES)分を検証する。

#[path = "../src/gpu/prelude.rs"]
#[allow(dead_code)]
mod prelude;

use paint_core::sim::CANVAS_SIZES;
use std::path::Path;

#[test]
fn all_shaders_compile() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/shaders");
    let common_src = std::fs::read_to_string(dir.join("common.wgsl")).expect("common.wgsl");

    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("assets/shaders")
        .filter_map(|e| {
            let path = e.ok()?.path();
            let name = path.file_name()?.to_str()?.to_owned();
            (path.extension().is_some_and(|x| x == "wgsl") && name != "common.wgsl").then_some(name)
        })
        .collect();
    names.sort();
    assert!(
        names.iter().any(|n| n == "display.wgsl"),
        "シェーダーが見つかりません"
    );

    for size in CANVAS_SIZES {
        let common = format!("{}{common_src}", prelude::shader_prelude(size));
        for name in &names {
            let src =
                std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            let full = format!("{common}\n{src}");
            let module = naga::front::wgsl::parse_str(&full).unwrap_or_else(|e| {
                panic!(
                    "{name}(size={size}): パースエラー:\n{}",
                    e.emit_to_string(&full)
                )
            });
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}(size={size}): 検証エラー: {e:?}"));
        }
    }
}
