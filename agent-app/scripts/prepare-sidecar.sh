#!/usr/bin/env bash
# 把工作区编译出来的 telesh 复制成 Tauri sidecar 需要的文件名。
#
# Tauri 要求 externalBin 的文件名带 target triple 后缀,而且路径相对 tauri.conf.json
# (即 src-tauri/),所以产物落在 src-tauri/binaries/:
#   src-tauri/binaries/telesh-x86_64-unknown-linux-gnu
# 打包时 Tauri 会去掉后缀,放在 app 二进制旁边(agent.rs 的 find_binary 会去找它)。
#
# 用法:
#   scripts/prepare-sidecar.sh            # release
#   scripts/prepare-sidecar.sh debug      # debug(配合 cargo tauri dev)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app="$(cd "$here/.." && pwd)"           # agent-app/
root="$(cd "$app/.." && pwd)"           # 仓库根

profile="${1:-release}"
triple="${2:-$(rustc -vV | awk '/^host:/ { print $2 }')}"

case "$triple" in
  *windows*) ext=".exe" ;;
  *)         ext="" ;;
esac

if [ "$profile" = "release" ]; then
  cargo build --manifest-path "$root/Cargo.toml" -p telesh --release
  built="$root/target/release/telesh$ext"
else
  cargo build --manifest-path "$root/Cargo.toml" -p telesh
  built="$root/target/debug/telesh$ext"
fi

mkdir -p "$app/src-tauri/binaries"
install -m 0755 "$built" "$app/src-tauri/binaries/telesh-$triple$ext"
echo "sidecar 就绪: agent-app/src-tauri/binaries/telesh-$triple$ext"
