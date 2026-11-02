#!/bin/sh
if [ "$1" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
    systemctl stop telesh-server.service >/dev/null 2>&1 || true
    systemctl disable telesh-server.service >/dev/null 2>&1 || true
fi
exit 0
