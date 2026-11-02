#!/usr/bin/env bash
# 从 CHANGELOG.md 抽取某个版本的条目，作为 GitHub Release 正文。
#
# 用法：
#   ./scripts/release-notes.sh 0.1.0-rc.1   # 打印该版本条目（不含 `## [...]` 标题）
#   ./scripts/release-notes.sh              # 打印当前清单版本对应的条目
#
# 条目格式见 Keep a Changelog：`## [<version>] - <date>` 起、到下一个 `## [` 止。
# 找不到该版本条目时以非零码退出（发布闸门据此拦截）。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${1:-}"
if [[ -z "$VERSION" ]]; then
    VERSION="$("$ROOT/scripts/check-release-version.sh")"
fi

CHANGELOG="$ROOT/CHANGELOG.md"
if [[ ! -f "$CHANGELOG" ]]; then
    echo "缺少 CHANGELOG.md" >&2
    exit 1
fi

notes="$(awk -v ver="## [$VERSION]" '
    index($0, ver) == 1 { inside = 1; next }
    inside && index($0, "## [") == 1 { exit }
    # 文件末尾的链接定义（`[x]: https://…`）不属于条目正文
    inside && /^\[[^]]+\]: / { exit }
    inside { print }
' "$CHANGELOG" | sed -e '/./,$!d' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}')"

if [[ -z "$notes" ]]; then
    echo "CHANGELOG.md 缺少版本 $VERSION 的条目（应为 '## [$VERSION] - <date>'）" >&2
    exit 1
fi

printf '%s\n' "$notes"
