#!/usr/bin/env bash
# 构建两个前端（agent-ui / server-ui）。
#
# rust-embed 在编译 telesh / telesh-server 时把 dist/ 嵌进二进制，所以打包前
# 必须先产出 dist/index.html。可用环境变量：
#   SKIP_WEB_BUILD=1  跳过构建，但要求两个 dist/index.html 已存在
#   PNPM=...          指定 pnpm 可执行文件（默认 pnpm）

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PNPM="${PNPM:-pnpm}"

if [[ "${SKIP_WEB_BUILD:-0}" != "1" ]]; then
    if ! command -v "$PNPM" >/dev/null 2>&1; then
        echo "缺少 pnpm（可用 PNPM=/path/to/pnpm 指定）" >&2
        exit 1
    fi
    for ui in agent-ui server-ui; do
        if [[ ! -d "$ROOT/$ui/node_modules" ]]; then
            "$PNPM" --dir "$ROOT/$ui" install --frozen-lockfile
        fi
        "$PNPM" --dir "$ROOT/$ui" build
    done
fi

for ui in agent-ui server-ui; do
    if [[ ! -f "$ROOT/$ui/dist/index.html" ]]; then
        echo "缺少 $ui/dist/index.html；先执行 ./scripts/build-ui.sh（或 cd $ui && pnpm build）" >&2
        exit 1
    fi
done

echo "前端就绪：agent-ui/dist、server-ui/dist"
