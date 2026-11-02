#!/bin/sh
# 首次安装给一份可编辑的配置；升级不覆盖。
if [ ! -e /etc/telesh/server.toml ] && [ -f /usr/share/telesh-server/server.toml.example ]; then
    install -m 0644 /usr/share/telesh-server/server.toml.example /etc/telesh/server.toml
fi
if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl enable telesh-server.service >/dev/null 2>&1 || true
    systemctl start telesh-server.service >/dev/null 2>&1 || true
fi
exit 0
