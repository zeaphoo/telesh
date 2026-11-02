#!/bin/sh
getent group telesh-server >/dev/null || groupadd -r telesh-server
getent passwd telesh-server >/dev/null || useradd -r -g telesh-server -d /var/lib/telesh-server -s /sbin/nologin -c "telesh server" telesh-server
install -d -o telesh-server -g telesh-server -m 0750 /var/lib/telesh-server
install -d -m 0755 /etc/telesh
exit 0
