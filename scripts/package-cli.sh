#!/usr/bin/env bash
# 构建独立命令行工具包：telesh（agent）与 telesh-server（server）。
#
# 不依赖 Tauri / GUI 栈；前端仍由 rust-embed 编进各自的二进制。
# 产物：
#   Linux/macOS  telesh_<ver>_<os>_<arch>.tar.gz
#                telesh-server_<ver>_<os>_<arch>.tar.gz
#   Windows      telesh_<ver>_windows_x64.zip
#                telesh-server_<ver>_windows_x64.zip
#
# 可用环境变量：
#   SKIP_WEB_BUILD=1  跳过前端构建（要求两个 dist 已存在）
#   DIST_DIR=...      输出目录，默认 <repo>/dist/cli

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib.sh
source "$ROOT/scripts/lib.sh"

DIST_DIR="${DIST_DIR:-$ROOT/dist/cli}"

require_cmd cargo node

case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*)
        OS=windows
        EXT=".exe"
        ARCH=x64
        require_cmd cygpath powershell.exe
        ;;
    Darwin)
        OS=macos
        EXT=""
        ARCH="$(uname -m)"
        # 别把 macOS 的扩展属性打成 ._ 文件。
        export COPYFILE_DISABLE=1
        ;;
    Linux)
        OS=linux
        EXT=""
        ARCH="$(linux_arch)"
        ;;
    *)
        echo "package-cli.sh 不支持的平台：$(uname -s)" >&2
        exit 1
        ;;
esac

"$ROOT/scripts/build-ui.sh"
version="$("$ROOT/scripts/check-release-version.sh")"

echo "构建 headless 二进制（前端已嵌入）…"
cargo build --release --locked -p telesh -p telesh-server

target_dir="$(cargo_target_dir "$ROOT/Cargo.toml")"
verify_ui_embedded "$target_dir/release/telesh${EXT}" "$ROOT/agent-ui/dist/index.html"
verify_ui_embedded "$target_dir/release/telesh-server${EXT}" "$ROOT/server-ui/dist/index.html"

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/.stage"

write_readme() {
    local path="$1" title="$2" bin="$3" example="$4" usage="$5" extra="$6"
    cat > "$path" <<TXT
${title} ${version} — standalone command-line tool
============================================================

Contents / 目录结构
  ${bin}${EXT}       ${title}
  ${example}         configuration example / 配置示例
  LICENSE            Apache-2.0
  README.txt         this file

Usage / 用法
${usage}

The web UI is embedded into ${bin}${EXT} at build time via rust-embed;
there are no separate web assets to install.
Web 前端已在编译期通过 rust-embed 嵌入 ${bin}${EXT}，不需要额外的静态资源目录。

${extra}

License / 许可证
  Apache-2.0 — see LICENSE
TXT
}

package_one() {
    local name="$1" example_src="$2" example_name="$3"
    local title="$4" usage="$5" extra="$6"
    local dirname="${name}_${version}_${OS}_${ARCH}"
    local stage="$DIST_DIR/.stage/$dirname"

    mkdir -p "$stage"
    cp -f "$target_dir/release/${name}${EXT}" "$stage/"
    cp -f "$example_src" "$stage/$example_name"
    cp -f "$ROOT/LICENSE" "$stage/LICENSE"
    write_readme "$stage/README.txt" "$title" "$name" "$example_name" "$usage" "$extra"

    if [[ "$OS" == "windows" ]]; then
        local zip="$DIST_DIR/${dirname}.zip"
        powershell.exe -NoProfile -NonInteractive -Command \
            "Compress-Archive -Path '$(cygpath -w "$stage")' -DestinationPath '$(cygpath -w "$zip")' -Force" >/dev/null
        echo "  $zip"
    else
        local tarball="$DIST_DIR/${dirname}.tar.gz"
        tar -C "$DIST_DIR/.stage" -czf "$tarball" "$dirname"
        echo "  $tarball"
    fi
}

package_one \
    telesh \
    "$ROOT/crates/telesh/config.toml.example" \
    config.toml.example \
    "telesh agent" \
    "  telesh --help
  telesh --dataroot ./data          # run the agent (dashboard :16345)
  telesh open                       # open the local dashboard in a browser" \
    "Config: <dataroot>/config.toml (created with defaults on first run).
  配置: <dataroot>/config.toml（首次运行自动生成）。"

package_one \
    telesh-server \
    "$ROOT/packaging/cli/server.toml.example" \
    server.toml.example \
    "telesh-server" \
    "  telesh-server --help
  telesh-server server.toml         # run the control server + edge ingress" \
    "Edit server.toml first (control_addr / ingress_addr / data_addr / dataroot).
  先按需修改 server.toml（控制面、入口、数据面地址与数据目录）。"

rm -rf "$DIST_DIR/.stage"

echo
echo "生成完成："
shopt -s nullglob
for file in "$DIST_DIR"/*.tar.gz "$DIST_DIR"/*.zip; do
    echo "  $file"
done
shopt -u nullglob
