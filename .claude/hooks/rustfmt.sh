#!/usr/bin/env bash
# PostToolUse(Edit / Write)フック: 書き換えた .rs ファイルに rustfmt をかける。
# rustfmt 単体は Cargo.toml の edition を読まないので 2024 を明示し、mise でリポジトリの Rust を使う。
f=$(jq -r '.tool_response.filePath // .tool_input.file_path // empty')
case "$f" in
  *.rs) ;;
  *) exit 0 ;;
esac
[ -f "$f" ] || exit 0
exec mise exec -C "${CLAUDE_PROJECT_DIR:-.}" -- rustfmt --edition 2024 "$f"
