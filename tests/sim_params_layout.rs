//! SimParams の Rust 定義(crates/paint-core/src/sim.rs)と WGSL 定義(assets/shaders/common.wgsl の
//! `struct SimParams`)が、フィールド名・順序・型(f32 / u32)で一致することの検査。
//! uniform buffer は bytemuck で生バイトのまま渡すので、片方だけ足す・順序を入れ替えると
//! 以降のフィールドが全部ずれる(コンパイルも naga 検証も通ってしまう)。
//!
//! Rust 側のフィールド並びは serde の直列化順(= 宣言順)から取る。SimParams は全フィールドが
//! f32 / u32 のフラットな構造体なので、整数表記なら u32、小数点か指数を含めば f32 と判定できる。

use paint_core::sim::SimParams;
use std::path::Path;

/// Rust 側: (フィールド名, 型) を宣言順に
fn rust_fields() -> Vec<(String, &'static str)> {
    let json = serde_json::to_string(&SimParams::default()).expect("SimParams の直列化");
    let body = json
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .expect("JSON オブジェクト");
    assert!(
        !body.contains(['{', '[']),
        "SimParams はスカラーだけのフラットな構造体の前提です"
    );
    body.split(',')
        .map(|kv| {
            let (key, value) = kv.split_once(':').expect("key:value");
            let key = key.trim_matches('"').to_owned();
            let ty = if value.contains(['.', 'e', 'E']) { "f32" } else { "u32" };
            (key, ty)
        })
        .collect()
}

/// WGSL 側: common.wgsl の `struct SimParams { ... };` の (フィールド名, 型) を宣言順に
fn wgsl_fields() -> Vec<(String, String)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/shaders/common.wgsl");
    let src = std::fs::read_to_string(&path).expect("common.wgsl");
    let start = src.find("struct SimParams {").expect("struct SimParams が見つかりません");
    let body = &src[start + "struct SimParams {".len()..];
    let end = body.find('}').expect("struct SimParams の終わり");
    body[..end]
        .lines()
        .map(|line| line.split("//").next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            let line = line.trim_end_matches(',');
            let (name, ty) = line
                .split_once(':')
                .unwrap_or_else(|| panic!("解釈できない行: {line}"));
            (name.trim().to_owned(), ty.trim().to_owned())
        })
        .collect()
}

#[test]
fn sim_params_matches_wgsl() {
    let rust = rust_fields();
    let wgsl = wgsl_fields();
    for (i, (r, w)) in rust.iter().zip(&wgsl).enumerate() {
        assert_eq!(
            (r.0.as_str(), r.1),
            (w.0.as_str(), w.1.as_str()),
            "{i} 番目のフィールドが一致しません(Rust 側 / common.wgsl 側)"
        );
    }
    assert_eq!(
        rust.len(),
        wgsl.len(),
        "フィールド数が一致しません(Rust {} / WGSL {})",
        rust.len(),
        wgsl.len()
    );
    // 全フィールド 4 バイト。直列化されないのは末尾の 16B 整列用パディング(serde skip)だけ
    let size = std::mem::size_of::<SimParams>();
    assert!(
        rust.len() * 4 <= size && size - rust.len() * 4 < 16,
        "直列化されないフィールドが途中にあります(フィールド {} 個 / {size} バイト)",
        rust.len()
    );
}
