#!/bin/sh
getent group telesh >/dev/null || groupadd -r telesh
getent passwd telesh >/dev/null || useradd -r -g telesh -d /var/lib/telesh -s /sbin/nologin -c "telesh agent" telesh
install -d -o telesh -g telesh -m 0750 /var/lib/telesh
exit 0
