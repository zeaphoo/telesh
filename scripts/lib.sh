#!/usr/bin/env bash
# 打包脚本共用的小工具。只应在 set -euo pipefail 之后 source。

# 某个 Cargo 清单对应的 target 目录（兼容自定义 CARGO_TARGET_DIR）。
cargo_target_dir() {
    local manifest="$1"
    cargo metadata --no-deps --format-version 1 --manifest-path "$manifest" |
        node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).target_directory))'
}

# dpkg / Linux 产物用的架构名（amd64/arm64，而不是 uname 的 x86_64/aarch64）。
linux_arch() {
    case "$(uname -m)" in
        x86_64 | amd64) echo amd64 ;;
        aarch64 | arm64) echo arm64 ;;
        *) uname -m ;;
    esac
}

require_cmd() {
    for command in "$@"; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "缺少命令: $command" >&2
            exit 1
        fi
    done
}

# 断言前端确实被 rust-embed 嵌进了二进制：从 dist/index.html 取出带 hash 的
# JS 资源名，再在二进制里找这个字符串（rust-embed 默认不压缩，直接可搜）。
verify_ui_embedded() {
    local bin="$1" index="$2"
    local asset
    asset="$(grep -o 'assets/[^"]*\.js' "$index" | head -n1)"
    if [[ -z "$asset" ]]; then
        echo "无法从 $index 解析出 JS 资源" >&2
        exit 1
    fi
    if ! grep -aqF "$asset" "$bin"; then
        echo "$bin 内没有找到前端资源 $asset（rust-embed 没生效？）" >&2
        exit 1
    fi
    echo "  嵌入校验通过：$(basename "$bin") <- $asset"
}
