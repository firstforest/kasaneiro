//! WGSL コンパイルエラーの行番号補正(R3 QoL)。gpu/mod.rs から分離した純関数群。
//!
//! common.wgsl を連結してコンパイルしている都合で、エラーの行番号はプレフィックス
//! (common + "\n")の行数分だけ後ろにずれる。エラー文字列中の行番号から `prefix_lines`
//! を引いて「シェーダーファイル内の行番号」に直す(ホットリロードの反復速度に直結する)。

/// naga/codespan の2形式を対象に、連結でずれた行番号を補正する:
/// - 位置指定 `wgsl:LINE:COL`
/// - コードフレームの行番号ガター `␠␠LINE␠│ …`(box-drawing `│` または ASCII `|`)
///
/// プレフィックス内(LINE ≤ prefix_lines = common.wgsl 側)のエラーはそのままにする。
/// 想定フォーマットに合わない行は変更しない(フォーマット変更に対する fail-safe)。
pub(super) fn remap_shader_error_lines(msg: &str, prefix_lines: usize) -> String {
    let shift = |n: usize| if n > prefix_lines { n - prefix_lines } else { n };
    msg.lines()
        .map(|line| remap_error_line(line, &shift))
        .collect::<Vec<_>>()
        .join("\n")
}

/// エラー1行分の行番号補正(remap_shader_error_lines のヘルパー)。
fn remap_error_line(line: &str, shift: &impl Fn(usize) -> usize) -> String {
    // 1) "wgsl:LINE:COL" の LINE
    if let Some(idx) = line.find("wgsl:") {
        let rest = &line[idx + "wgsl:".len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<usize>() {
            return format!(
                "{}wgsl:{}{}",
                &line[..idx],
                shift(n),
                &rest[digits.len()..]
            );
        }
    }
    // 2) 行番号ガター "␠…␠LINE␠…│…" の LINE(数字の直後に空白を挟んで │ / | が来る行だけ)
    let indent: usize = line.len() - line.trim_start().len();
    let body = &line[indent..];
    let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &body[digits.len()..];
        if after.trim_start().starts_with(['│', '|'])
            && let Ok(n) = digits.parse::<usize>()
        {
            return format!("{}{}{}", &line[..indent], shift(n), after);
        }
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// common.wgsl 連結でずれた行番号が、位置指定・ガターの両方で補正されること(R3)
    #[test]
    fn remaps_concatenated_error_lines() {
        let prefix = 30; // common.wgsl が 30 行を占めると仮定
        let msg = "parsing error: unknown identifier\n  ┌─ wgsl:37:13\n   │\n37 │     let x = foo;\n   │             ^^^ oops";
        let out = remap_shader_error_lines(msg, prefix);
        assert!(out.contains("wgsl:7:13"), "位置指定が未補正: {out}");
        assert!(out.contains("7 │     let x = foo;"), "ガターが未補正: {out}");
        // カラム番号(13)や caret 行(^^^)は触らない
        assert!(out.contains(":13"));
        assert!(out.contains("^^^ oops"));
    }

    /// プレフィックス内(common.wgsl 側)のエラー行番号は据え置く
    #[test]
    fn keeps_common_region_lines() {
        let prefix = 30;
        let msg = "  ┌─ wgsl:10:3";
        assert_eq!(remap_shader_error_lines(msg, prefix), "  ┌─ wgsl:10:3");
    }

    /// 行番号を含まない普通のメッセージは素通し(fail-safe)
    #[test]
    fn passes_through_plain_text() {
        let msg = "Shader validation error: something went wrong";
        assert_eq!(remap_shader_error_lines(msg, 30), msg);
    }
}
