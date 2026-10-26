#!/bin/sh
set -e

# Sync static web export to shared volume if target directory is mounted
if [ -d "/srv/web" ] && [ -d "/var/www/html" ]; then
    cp -r /srv/web/. /var/www/html/ 2>/dev/null || true
fi

exec "$@"
