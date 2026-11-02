#!/usr/bin/env bash
# 构建 Linux headless 服务安装包：agent（telesh）+ server（telesh-server）的 deb + rpm。
#
# 依赖：cargo、pnpm、cargo-deb、cargo-generate-rpm。
# 可用环境变量：
#   SKIP_WEB_BUILD=1  跳过前端构建（要求 agent-ui/dist、server-ui/dist 已存在）
#   DIST_DIR=...      输出目录，默认 <repo>/dist/linux
#
# 两个前端都通过 rust-embed 嵌入各自的二进制，包内没有散落的 web 资源。
# 单元文件：packaging/systemd/{telesh,telesh-server}.service
#   agent 单元默认不 enable（先配置 control_server）；
#   server 单元安装后 enable + start。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib.sh
source "$ROOT/scripts/lib.sh"

DIST_DIR="${DIST_DIR:-$ROOT/dist/linux}"

require_cmd cargo

if ! cargo deb --version >/dev/null 2>&1; then
    echo "缺少 cargo-deb；请执行: cargo install cargo-deb --locked" >&2
    exit 1
fi
if ! cargo generate-rpm --version >/dev/null 2>&1; then
    echo "缺少 cargo-generate-rpm；请执行: cargo install cargo-generate-rpm --locked" >&2
    exit 1
fi

"$ROOT/scripts/build-ui.sh"

cd "$ROOT"
version="$("$ROOT/scripts/check-release-version.sh")"
# RPM 的 Version 不允许 `-`（semver 预发布必须带），映射成 `~`，排序仍低于正式版。
rpm_version="${version/-/\~}"

echo "构建 headless 二进制（前端已嵌入）…"
cargo build --release --locked -p telesh -p telesh-server

target_dir="$(cargo_target_dir "$ROOT/Cargo.toml")"
verify_ui_embedded "$target_dir/release/telesh" "$ROOT/agent-ui/dist/index.html"
verify_ui_embedded "$target_dir/release/telesh-server" "$ROOT/server-ui/dist/index.html"

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

for package in telesh telesh-server; do
    echo "打包 deb：$package"
    cargo deb -p "$package" --no-build --no-dbgsym --output "$DIST_DIR"
done

for crate_dir in crates/telesh crates/telesh-server; do
    echo "打包 rpm：$crate_dir"
    cargo generate-rpm -p "$crate_dir" --output "$DIST_DIR" \
        --set-metadata "version = \"$rpm_version\""
done

# 产物名带 OS 标记，便于在 Release 资产列表里辨认。
# deb 在最后一个 `_` 前插入，rpm 在最后一个 `.` 前插入；包内元数据不受影响。
for file in "$DIST_DIR"/telesh_*.deb "$DIST_DIR"/telesh-server_*.deb; do
    [[ -f "$file" ]] || continue
    dir="$(dirname "$file")"
    name="$(basename "$file" .deb)"
    mv -f "$file" "$dir/${name%_*}_linux_${name##*_}.deb"
done
for file in "$DIST_DIR"/telesh-*.rpm; do
    [[ -f "$file" ]] || continue
    dir="$(dirname "$file")"
    name="$(basename "$file" .rpm)"
    mv -f "$file" "$dir/${name%.*}.linux.${name##*.}.rpm"
done

# 断言：二进制在包里、前端确实被嵌入（release 构建已由 build.rs 保证），
# 单元文件落在预期路径。
if command -v dpkg-deb >/dev/null 2>&1; then
    check_deb() {
        local deb="$1" bin="$2" unit="$3"
        local listing
        listing="$(dpkg-deb -c "$deb")"
        grep -qE "\./usr/bin/${bin}$" <<<"$listing" || {
            echo "deb 内缺少 /usr/bin/${bin}: $deb" >&2
            exit 1
        }
        grep -qE "\./usr/lib/systemd/system/${unit}$" <<<"$listing" || {
            echo "deb 内缺少 systemd 单元 ${unit}: $deb" >&2
            exit 1
        }
    }
    for deb in "$DIST_DIR"/telesh_*.deb; do
        [[ -f "$deb" ]] || continue
        check_deb "$deb" telesh telesh.service
    done
    for deb in "$DIST_DIR"/telesh-server_*.deb; do
        [[ -f "$deb" ]] || continue
        check_deb "$deb" telesh-server telesh-server.service
    done
fi

echo
echo "生成完成："
find "$DIST_DIR" -maxdepth 1 -type f \( -name '*.deb' -o -name '*.rpm' \) -printf '  %p\n' | sort
