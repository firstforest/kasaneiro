#!/usr/bin/env bash
# PostToolUse(Bash)フック: シェル経由(Python・sed・cargo fmt 以外のツール)で書き換えた .rs も整形する。
# Edit / Write の hook(rustfmt.sh)は Bash での書き換えでは発火しないので、
# 作業ツリーで HEAD から変更・追加された .rs だけに rustfmt をかける(変えていないファイルには触れない)。
root=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
cd "$root" || exit 0
mapfile -t files < <(
  {
    git diff --name-only --diff-filter=ACMR HEAD -- '*.rs'
    git ls-files --others --exclude-standard -- '*.rs'
  } | sort -u
)
[ ${#files[@]} -eq 0 ] && exit 0
exec mise exec -- rustfmt --edition 2024 "${files[@]}"
