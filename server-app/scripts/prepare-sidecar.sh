#!/usr/bin/env bash
# 把工作区编译出来的 telesh-server 复制成 Tauri sidecar 需要的文件名。
#
# Tauri 要求 externalBin 的文件名带 target triple 后缀,而且路径相对 tauri.conf.json
# (即 src-tauri/),所以产物落在 src-tauri/binaries/:
#   src-tauri/binaries/telesh-server-x86_64-unknown-linux-gnu
# 打包时 Tauri 会去掉后缀,放在 app 二进制旁边(server.rs 的 find_binary 会去找它)。
#
# 用法:
#   scripts/prepare-sidecar.sh            # release
#   scripts/prepare-sidecar.sh debug      # debug(配合 cargo tauri dev)
#
# 注意:**改过 crates/telesh-server 之后必须重跑本脚本**,否则 app 拉起的是旧二进制。
# server 的前端在编译期内嵌,所以还要先 `(cd server-ui && pnpm build)`。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app="$(cd "$here/.." && pwd)"           # server-app/
root="$(cd "$app/.." && pwd)"           # 仓库根

profile="${1:-release}"
triple="${2:-$(rustc -vV | awk '/^host:/ { print $2 }')}"

case "$triple" in
  *windows*) ext=".exe" ;;
  *)         ext="" ;;
esac

if [ "$profile" = "release" ]; then
  cargo build --manifest-path "$root/Cargo.toml" -p telesh-server --release
  built="$root/target/release/telesh-server$ext"
else
  cargo build --manifest-path "$root/Cargo.toml" -p telesh-server
  built="$root/target/debug/telesh-server$ext"
fi

mkdir -p "$app/src-tauri/binaries"
install -m 0755 "$built" "$app/src-tauri/binaries/telesh-server-$triple$ext"
echo "sidecar 就绪: server-app/src-tauri/binaries/telesh-server-$triple$ext"
