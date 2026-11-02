#!/usr/bin/env bash
# 构建 Windows 桌面客户端：agent-app 与 server-app 的 NSIS / MSI + 便携 zip。
#
# 依赖：cargo、pnpm、node、cargo-tauri；在 Windows 的 Git Bash / MSYS 下运行。
# 可用环境变量：
#   SKIP_WEB_BUILD=1     跳过前端构建（要求两个 dist 已存在）
#   DIST_DIR=...         输出目录，默认 <repo>/dist/windows
#   TAURI_BUNDLES=...    传给 tauri 的 bundle 列表，默认 nsis,msi
#                        （没有 WiX 下载条件时可设 TAURI_BUNDLES=nsis）
#
# 每个 app 的主程序（telesh-app.exe / telesh-server-app.exe）与 sidecar
# （telesh.exe / telesh-server.exe）必须同目录：安装包由 Tauri externalBin 处理，
# 便携 zip 里两个 exe 并排。sidecar 自身已用 rust-embed 嵌入前端。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib.sh
source "$ROOT/scripts/lib.sh"

DIST_DIR="${DIST_DIR:-$ROOT/dist/windows}"
BUNDLES="${TAURI_BUNDLES:-nsis,msi}"

case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) ;;
    *)
        echo "package-windows.sh 仅在 Windows（Git Bash / MSYS）上运行" >&2
        exit 1
        ;;
esac

require_cmd cargo node cygpath

if ! cargo tauri --version >/dev/null 2>&1; then
    echo "缺少 tauri-cli；请执行: cargo install tauri-cli --locked" >&2
    exit 1
fi

"$ROOT/scripts/build-ui.sh"
version="$("$ROOT/scripts/check-release-version.sh")"

"$ROOT/agent-app/scripts/prepare-sidecar.sh" release
"$ROOT/server-app/scripts/prepare-sidecar.sh" release

# 随包 sidecar 必须真的带前端（rust-embed），否则客户端打开的是 404 面板。
ROOT_TARGET="$(cargo_target_dir "$ROOT/Cargo.toml")"
verify_ui_embedded "$ROOT_TARGET/release/telesh.exe" "$ROOT/agent-ui/dist/index.html"
verify_ui_embedded "$ROOT_TARGET/release/telesh-server.exe" "$ROOT/server-ui/dist/index.html"

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

# MSI（WiX）的 ProductVersion 只能是数字 `major.minor.patch[.build]`；预发布
# 版本把第一个数字映射到 build 段（0.1.0-rc.1 → 0.1.0.1）。
msi_version=""
if [[ "$version" == *-* ]]; then
    core="${version%%-*}"
    pre="${version#*-}"
    build=1
    if [[ "$pre" =~ [0-9]+ ]]; then
        build="$((10#${BASH_REMATCH[0]}))"
    fi
    (( build > 65535 )) && build=65535
    msi_version="${core}.${build}"
fi

build_app() {
    local app="$1" pkg="$2" main_exe="$3" sidecar_src="$4" sidecar_exe="$5"
    local tauri_dir="$ROOT/$app/src-tauri"
    local inject_config="$tauri_dir/.bundle-resources.json"

    echo "构建 Tauri 客户端：$app"
    local app_target
    app_target="$(cargo_target_dir "$tauri_dir/Cargo.toml")"
    # 清掉上次的安装包，避免旧产物混进本次生成。
    rm -rf "$app_target/release/bundle/nsis" "$app_target/release/bundle/msi"

    if [[ -n "$msi_version" ]]; then
        # 只注入 WiX 版本；sidecar 由 tauri.conf.json 的 externalBin 处理。
        node -e '
const fs = require("fs")
const [out, msi] = process.argv.slice(1)
fs.writeFileSync(out, JSON.stringify({ bundle: { windows: { wix: { version: msi } } } }))
' "$(cygpath -w "$inject_config")" "$msi_version"
    fi

    local tauri_args=(build --bundles "$BUNDLES")
    if [[ -n "$msi_version" ]]; then
        tauri_args+=(--config "$(cygpath -w "$inject_config")")
    fi
    (cd "$tauri_dir" && cargo tauri "${tauri_args[@]}")
    rm -f "$inject_config"

    local bundle_dir app_exe
    bundle_dir="$app_target/release/bundle"
    app_exe="$app_target/release/$main_exe"
    if [[ ! -f "$app_exe" ]]; then
        echo "未找到应用可执行文件 $app_exe" >&2
        exit 1
    fi

    # 便携包：主程序 + sidecar 同目录，免安装。
    local portable="$DIST_DIR/portable-$app"
    rm -rf "$portable"
    mkdir -p "$portable"
    cp -f "$app_exe" "$portable/$main_exe"
    cp -f "$ROOT_TARGET/release/$sidecar_src" "$portable/$sidecar_exe"
    cat > "$portable/README.txt" <<TXT
telesh ${app} ${version} — 便携版 / portable
================================================

启动 / Run
  双击 ${main_exe}。免安装、免管理员权限；托盘图标常驻。
  Double-click ${main_exe}. No installer or admin rights needed.

目录结构 / Layout
  ${main_exe}      桌面应用 / desktop app
  ${sidecar_exe}   后端（必须与主程序同目录）/ backend (must stay next to the app)
  README.txt       本说明 / this file

前端资源已编译进 ${sidecar_exe}，没有单独的 web 资源目录。
The web UI is embedded into ${sidecar_exe}; there is no separate asset folder.

卸载 / Uninstall
  删除本目录即可。数据目录由应用自身管理（应用配置目录）。
  Delete this folder. Application data lives in the app config directory.

未签名 / Unsigned build
  本包未做代码签名，SmartScreen 可能提示「未知发布者」。
  Not code-signed: SmartScreen may warn about an unknown publisher.
TXT

    # 便携包断言：两个 exe 都必须存在（Windows 文件名不区分大小写）。
    local name
    for name in "$main_exe" "$sidecar_exe"; do
        if [[ ! -f "$portable/$name" ]]; then
            echo "便携包缺少 ${name}" >&2
            exit 1
        fi
    done

    local zip="$DIST_DIR/${pkg}_${version}_windows_x64_portable.zip"
    powershell.exe -NoProfile -NonInteractive -Command \
        "Compress-Archive -Path '$(cygpath -w "$portable")\\*' -DestinationPath '$(cygpath -w "$zip")' -Force" >/dev/null
    rm -rf "$portable"

    # 安装包：nsis/*.exe 与 msi/*.msi。
    shopt -s nullglob
    local installers=("$bundle_dir"/nsis/*.exe "$bundle_dir"/msi/*.msi)
    shopt -u nullglob
    if [[ ${#installers[@]} -eq 0 ]]; then
        echo "Tauri bundler 未产出安装包（bundle 目录：$bundle_dir，bundles=$BUNDLES）" >&2
        exit 1
    fi

    local file out locale
    for file in "${installers[@]}"; do
        case "$file" in
            *.exe) out="$DIST_DIR/${pkg}_${version}_windows_x64-setup.exe" ;;
            *.msi)
                locale="${file##*_}"
                out="$DIST_DIR/${pkg}_${version}_windows_x64_${locale}"
                ;;
            *) out="$DIST_DIR/$(basename "$file")" ;;
        esac
        cp -f "$file" "$out"
    done

    echo "  $zip"
}

build_app agent-app telesh-agent-app telesh-app.exe telesh.exe telesh-sidecar.exe
build_app server-app telesh-server-app telesh-server-app.exe telesh-server.exe telesh-server-sidecar.exe

# NSIS 内容断言：安装包里必须同时有主程序与 sidecar（找不到 7-Zip 时只告警）。
SEVEN_ZIP=""
if command -v 7z >/dev/null 2>&1; then
    SEVEN_ZIP="7z"
elif [[ -x "/c/Program Files/7-Zip/7z.exe" ]]; then
    SEVEN_ZIP="/c/Program Files/7-Zip/7z.exe"
fi
for file in "$DIST_DIR"/*-setup.exe; do
    [[ -f "$file" ]] || continue
    if [[ -z "$SEVEN_ZIP" ]]; then
        echo "WARN: 未找到 7-Zip，跳过 ${file##*/} 的内容断言" >&2
        continue
    fi
    listing="$("$SEVEN_ZIP" l "$file" 2>/dev/null || true)"
    expected=()
    case "$file" in
        *telesh-agent-app*) expected=("telesh-app.exe" "telesh-sidecar.exe") ;;
        *telesh-server-app*) expected=("telesh-server-app.exe" "telesh-server-sidecar.exe") ;;
    esac
    for name in "${expected[@]}"; do
        if ! grep -qi "$name" <<<"$listing"; then
            echo "安装包 ${file##*/} 内缺少 $name" >&2
            exit 1
        fi
    done
done

echo
echo "生成完成："
find "$DIST_DIR" -maxdepth 1 -type f -printf '  %p\n' | sort
