#!/usr/bin/env bash
# 构建 macOS 桌面客户端：agent-app 与 server-app 的 .app + .dmg。
#
# 依赖：cargo、pnpm、node、cargo-tauri、codesign、hdiutil（均随 Xcode/macOS）。
# 可用环境变量：
#   SKIP_WEB_BUILD=1  跳过前端构建（要求两个 dist 已存在）
#   DIST_DIR=...      输出目录，默认 <repo>/dist/mac
#
# 签名/公证（可选，全部缺省时产出未签名 dmg）：
#   MAC_CERT_P12_BASE64          Developer ID Application 证书 p12（base64）
#   MAC_CERT_PASSWORD            p12 口令
#   MAC_SIGN_IDENTITY            签名身份全名（也用于本机钥匙串里的身份）
#   MAC_NOTARY_API_KEY_ID        App Store Connect API key id
#   MAC_NOTARY_API_ISSUER_ID     App Store Connect issuer id
#   MAC_NOTARY_API_KEY_P8_BASE64 API key 的 .p8 私钥（base64）
#
# 这些变量会映射成 Tauri 认识的 APPLE_* 变量；Tauri 负责导入临时钥匙串、
# hardened runtime 签名、公证与钉票。每个 app 的 sidecar（telesh-sidecar / telesh-server-sidecar）
# 由 externalBin 打进 Contents/MacOS/，签名覆盖它；sidecar 自身已嵌入前端。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib.sh
source "$ROOT/scripts/lib.sh"

DIST_DIR="${DIST_DIR:-$ROOT/dist/mac}"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "package-mac.sh 仅支持 macOS" >&2
    exit 1
fi

require_cmd cargo node codesign hdiutil

if ! cargo tauri --version >/dev/null 2>&1; then
    echo "缺少 tauri-cli；请执行: cargo install tauri-cli --locked" >&2
    exit 1
fi

"$ROOT/scripts/build-ui.sh"
version="$("$ROOT/scripts/check-release-version.sh")"

"$ROOT/agent-app/scripts/prepare-sidecar.sh" release
"$ROOT/server-app/scripts/prepare-sidecar.sh" release

# 随包 sidecar 必须真的带前端（rust-embed），否则客户端打开的是 404 面板。
root_target="$(cargo_target_dir "$ROOT/Cargo.toml")"
verify_ui_embedded "$root_target/release/telesh" "$ROOT/agent-ui/dist/index.html"
verify_ui_embedded "$root_target/release/telesh-server" "$ROOT/server-ui/dist/index.html"

# 把仓库约定的 MAC_* secrets 映射成 Tauri 的 APPLE_* 变量。
if [[ -n "${MAC_CERT_P12_BASE64:-}" && -z "${MAC_CERT_PASSWORD:-}" ]]; then
    echo "配置了 MAC_CERT_P12_BASE64，但缺少 MAC_CERT_PASSWORD" >&2
    exit 1
fi
if [[ -n "${MAC_NOTARY_API_KEY_P8_BASE64:-}" \
    && -z "${MAC_SIGN_IDENTITY:-}" && -z "${MAC_CERT_P12_BASE64:-}" ]]; then
    echo "配置了公证凭据，但既没有 MAC_SIGN_IDENTITY 也没有 MAC_CERT_P12_BASE64，无法签名" >&2
    exit 1
fi
if [[ -n "${MAC_CERT_P12_BASE64:-}" ]]; then
    export APPLE_CERTIFICATE="$MAC_CERT_P12_BASE64"
    export APPLE_CERTIFICATE_PASSWORD="${MAC_CERT_PASSWORD:-}"
fi
if [[ -n "${MAC_SIGN_IDENTITY:-}" ]]; then
    export APPLE_SIGNING_IDENTITY="$MAC_SIGN_IDENTITY"
fi

notary_key=""
cleanup() {
    if [[ -n "$notary_key" ]]; then
        rm -f "$notary_key"
    fi
}
trap cleanup EXIT

if [[ -n "${MAC_NOTARY_API_KEY_P8_BASE64:-}" ]]; then
    if [[ -z "${MAC_NOTARY_API_KEY_ID:-}" || -z "${MAC_NOTARY_API_ISSUER_ID:-}" ]]; then
        echo "配置了 MAC_NOTARY_API_KEY_P8_BASE64，但缺少 MAC_NOTARY_API_KEY_ID / MAC_NOTARY_API_ISSUER_ID" >&2
        exit 1
    fi
    notary_key="$(mktemp "${TMPDIR:-/tmp}/telesh-notary-key.XXXXXX")"
    umask 077
    printf '%s' "$MAC_NOTARY_API_KEY_P8_BASE64" | base64 -d > "$notary_key"
    export APPLE_API_KEY="$MAC_NOTARY_API_KEY_ID"
    export APPLE_API_ISSUER="$MAC_NOTARY_API_ISSUER_ID"
    export APPLE_API_KEY_PATH="$notary_key"
fi

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

ARCH="$(uname -m)"

build_app() {
    local app="$1" pkg="$2" sidecar="$3"
    local tauri_dir="$ROOT/$app/src-tauri"

    echo "构建 Tauri 客户端：$app"
    local app_target
    app_target="$(cargo_target_dir "$tauri_dir/Cargo.toml")"
    # 清掉上次的 .app/.dmg，避免把旧产物当成本次结果。
    rm -rf "$app_target/release/bundle/macos" "$app_target/release/bundle/dmg"
    (cd "$tauri_dir" && cargo tauri build --bundles app,dmg)

    local bundle_dir app_bundle dmg
    bundle_dir="$app_target/release/bundle"

    shopt -s nullglob
    local app_bundles=("$bundle_dir"/macos/*.app)
    local dmgs=("$bundle_dir"/dmg/*.dmg)
    shopt -u nullglob

    if [[ ${#app_bundles[@]} -ne 1 || ${#dmgs[@]} -ne 1 ]]; then
        echo "Tauri bundler 期望产出 1 个 .app 和 1 个 .dmg（bundle 目录：$bundle_dir）" >&2
        exit 1
    fi
    app_bundle="${app_bundles[0]}"
    dmg="${dmgs[0]}"

    # sidecar 必须在 Contents/MacOS/ 下；找不到说明 externalBin 没打进去。
    if [[ ! -f "$app_bundle/Contents/MacOS/$sidecar" ]]; then
        echo "$app_bundle 内缺少 sidecar Contents/MacOS/$sidecar" >&2
        exit 1
    fi

    if [[ -n "${MAC_SIGN_IDENTITY:-}" ]]; then
        codesign --verify --deep --strict "$app_bundle"
        echo "签名校验通过：$app_bundle"
    else
        echo "WARN: 未配置 MAC_SIGN_IDENTITY，产物未签名（$app_bundle）" >&2
    fi

    local out="$DIST_DIR/${pkg}_${version}_macos_${ARCH}.dmg"
    cp -f "$dmg" "$out"
    echo "  $out"
}

build_app agent-app telesh-agent-app telesh-sidecar
build_app server-app telesh-server-app telesh-server-sidecar

echo
echo "生成完成："
find "$DIST_DIR" -maxdepth 1 -type f -name '*.dmg' -print | sort
