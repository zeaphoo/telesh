#!/bin/sh
# 首次安装给一份可编辑的配置；升级不覆盖。
if [ ! -e /var/lib/telesh/config.toml ] && [ -f /usr/share/telesh/config.toml.example ]; then
    install -o telesh -g telesh -m 0640 /usr/share/telesh/config.toml.example /var/lib/telesh/config.toml
fi
# 单元默认不启用：先让用户配置 control_server，再 systemctl enable --now telesh。
if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload >/dev/null 2>&1 || true
fi
exit 0
