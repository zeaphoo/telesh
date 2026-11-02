#!/usr/bin/env bash
# 构建 Linux 桌面客户端（Tauri）：agent-app 与 server-app 的 deb + AppImage。
#
# 依赖：cargo、pnpm、node、cargo-tauri、Tauri 2 的 Linux 系统依赖
#       （webkit2gtk-4.1 / libayatana-appindicator3 / librsvg2 / patchelf 等）。
# 可用环境变量：
#   SKIP_WEB_BUILD=1  跳过前端构建（要求两个 dist 已存在）
#   DIST_DIR=...      输出目录，默认 <repo>/dist/linux-app
#   LINUX_APP_TAURI_BUNDLES=...  Tauri bundle 列表，默认 deb,appimage
#                        （受限网络/无 FUSE 的环境可只出 deb：deb）
#
# 每个 app 都把随包的 headless 二进制作为 Tauri externalBin 打进安装目录
# （/usr/bin/<sidecar>），而该二进制本身已用 rust-embed 嵌入前端。因此安装包
# 不依赖外部 web 资源，也没有单独的「前端资源目录」。
#
# Tauri 的 deb 包名由 productName 推导，会与 headless 服务包重名/混淆，所以这里
# 重写 control 里的 Package：telesh-agent-app / telesh-server-app。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib.sh
source "$ROOT/scripts/lib.sh"

DIST_DIR="${DIST_DIR:-$ROOT/dist/linux-app}"
BUNDLES="${LINUX_APP_TAURI_BUNDLES:-deb,appimage}"

require_cmd cargo node dpkg-deb

if ! cargo tauri --version >/dev/null 2>&1; then
    echo "缺少 tauri-cli；请执行: cargo install tauri-cli --locked" >&2
    exit 1
fi

"$ROOT/scripts/build-ui.sh"
version="$("$ROOT/scripts/check-release-version.sh")"

# 先准备 sidecar（内部会 cargo build --release -p telesh / telesh-server）。
"$ROOT/agent-app/scripts/prepare-sidecar.sh" release
"$ROOT/server-app/scripts/prepare-sidecar.sh" release

# 随包 sidecar 必须真的带前端（rust-embed），否则客户端打开的是 404 面板。
root_target="$(cargo_target_dir "$ROOT/Cargo.toml")"
verify_ui_embedded "$root_target/release/telesh" "$ROOT/agent-ui/dist/index.html"
verify_ui_embedded "$root_target/release/telesh-server" "$ROOT/server-ui/dist/index.html"

export APPIMAGE_EXTRACT_AND_RUN=1
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

build_app() {
    local app="$1" pkg="$2" sidecar="$3"
    local tauri_dir="$ROOT/$app/src-tauri"

    echo "构建 Tauri 客户端：$app（bundles=$BUNDLES）"
    local target_root
    target_root="$(cargo_target_dir "$tauri_dir/Cargo.toml")"
    # linuxdeploy 会复用 bundle/appimage 下的 AppDir；上次中断留下的坏 AppDir
    # 会让下一次构建报 `failed to run linuxdeploy`，先清掉。
    rm -rf "$target_root/release/bundle/appimage" \
           "$target_root/release/bundle/appimage_deb" \
           "$target_root/release/bundle/deb"

    (cd "$tauri_dir" && cargo tauri build --bundles "$BUNDLES")

    local bundle_dir="$target_root/release/bundle"

    shopt -s nullglob
    local debs=("$bundle_dir"/deb/*.deb)
    local appimages=("$bundle_dir"/appimage/*.AppImage)
    shopt -u nullglob

    if [[ ${#debs[@]} -eq 0 ]]; then
        echo "Tauri bundler 未产出 deb（bundle 目录：$bundle_dir）" >&2
        exit 1
    fi
    if [[ ",$BUNDLES," == *",appimage,"* && ${#appimages[@]} -eq 0 ]]; then
        echo "Tauri bundler 未产出 AppImage（bundle 目录：$bundle_dir）" >&2
        exit 1
    fi

    local arch
    arch="$(linux_arch)"

    local file
    for file in "${appimages[@]}"; do
        cp -f "$file" "$DIST_DIR/${pkg}_${version}_linux_${arch}.AppImage"
    done

    for file in "${debs[@]}"; do
        local deb_version deb_arch work out
        deb_version="$(dpkg-deb -f "$file" Version)"
        deb_arch="$(dpkg-deb -f "$file" Architecture)"
        work="$(mktemp -d)"
        dpkg-deb -R "$file" "$work"
        sed -i "s/^Package: .*/Package: ${pkg}/" "$work/DEBIAN/control"
        # Debian 用 `~` 表达预发布，保证 0.1.0~rc.1 < 0.1.0；与 cargo-deb 的
        # 服务包保持一致（Tauri 会原样保留 -rc.1）。
        if [[ "$deb_version" == *-* ]]; then
            deb_version="${deb_version/-/\~}"
            sed -i "s/^Version: .*/Version: ${deb_version}/" "$work/DEBIAN/control"
        fi
        out="$DIST_DIR/${pkg}_${deb_version}_linux_${deb_arch}.deb"
        dpkg-deb --root-owner-group -b "$work" "$out" >/dev/null
        rm -rf "$work"

        if [[ "$(dpkg-deb -f "$out" Package)" != "$pkg" ]]; then
            echo "deb 包名应为 ${pkg}，实际：$(dpkg-deb -f "$out" Package)" >&2
            exit 1
        fi
        # sidecar 必须真的在包里，否则 app 运行时会找不到后端。
        local listing
        listing="$(dpkg-deb -c "$out")"
        grep -qE "\./usr/bin/${sidecar}$" <<<"$listing" || {
            echo "deb 内缺少 sidecar /usr/bin/${sidecar}: $out" >&2
            exit 1
        }
        echo "  $out  (Package: ${pkg}, sidecar: ${sidecar})"
        # Tauri 的中间产物按 productName 命名，已被重打包取代。
        rm -f "$file"
    done
}

build_app agent-app telesh-agent-app telesh-sidecar
build_app server-app telesh-server-app telesh-server-sidecar

echo
echo "生成完成："
for file in "$DIST_DIR"/*.deb; do
    echo "  $file  (Package: $(dpkg-deb -f "$file" Package))"
done
shopt -s nullglob
for file in "$DIST_DIR"/*.AppImage; do
    echo "  $file"
done
shopt -u nullglob
