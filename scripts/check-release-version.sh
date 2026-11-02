#!/usr/bin/env bash
# 发布版本一致性闸门：所有清单必须一致；给了 tag 时还要 v<version> == tag。
#
# 用法：
#   ./scripts/check-release-version.sh            # 只校验清单，打印裸版本
#   ./scripts/check-release-version.sh v0.1.0     # 额外校验 tag
#
# 成功时把裸版本打到 stdout（CI 作为 job output 使用），失败以非零码退出。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG="${1:-}"

if [[ -n "$TAG" && "$TAG" != v* ]]; then
    echo "标签必须形如 v<version>，收到：$TAG" >&2
    exit 1
fi

# 根 Cargo.toml 的 [workspace.package] version（crates/* 都继承它）。
workspace_version="$(awk -F'"' '
    /^\[workspace\.package\]$/ { inpkg = 1; next }
    /^\[/ { inpkg = 0 }
    inpkg && /^version[[:space:]]*=/ { print $2; exit }
' "$ROOT/Cargo.toml")"
if [[ -z "$workspace_version" ]]; then
    echo "无法读取 Cargo.toml 的 [workspace.package] version" >&2
    exit 1
fi

failed=0

json_version() {
    sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*$/\1/p' "$1" | head -n1
}

cargo_version() {
    awk -F'"' '/^version = "/{print $2; exit}' "$1"
}

check() {
    local label="$1" actual="$2"
    if [[ "$actual" != "$workspace_version" ]]; then
        echo "版本不一致：Cargo.toml=$workspace_version, ${label}=${actual:-<空>}" >&2
        failed=1
    fi
}

for manifest in \
    "$ROOT/agent-app/src-tauri/Cargo.toml" \
    "$ROOT/server-app/src-tauri/Cargo.toml"; do
    if [[ ! -f "$manifest" ]]; then
        echo "缺少清单：$manifest" >&2
        failed=1
        continue
    fi
    check "${manifest#"$ROOT"/}" "$(cargo_version "$manifest")"
done

for conf in \
    "$ROOT/agent-app/src-tauri/tauri.conf.json" \
    "$ROOT/server-app/src-tauri/tauri.conf.json"; do
    if [[ ! -f "$conf" ]]; then
        echo "缺少配置：$conf" >&2
        failed=1
        continue
    fi
    check "${conf#"$ROOT"/}" "$(json_version "$conf")"
done

for pkg in "$ROOT/agent-ui/package.json" "$ROOT/server-ui/package.json"; do
    if [[ ! -f "$pkg" ]]; then
        echo "缺少清单：$pkg" >&2
        failed=1
        continue
    fi
    check "${pkg#"$ROOT"/}" "$(json_version "$pkg")"
done

if [[ "$failed" -ne 0 ]]; then
    exit 1
fi

if [[ -n "$TAG" && "$TAG" != "v$workspace_version" ]]; then
    echo "标签与版本不一致：tag=$TAG, 清单期望=v$workspace_version" >&2
    exit 1
fi

echo "$workspace_version"
