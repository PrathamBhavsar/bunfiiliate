#!/usr/bin/env bash
# Run the tiny-tmpfs spool test (Linux, needs root or CAP_SYS_ADMIN: use `docker run --privileged`).
# macOS and unprivileged runs skip with a message.
set -euo pipefail
if [ "$(uname)" != Linux ]; then echo "tmpfs-test: skipped (not Linux)"; exit 0; fi
if [ "$(id -u)" != 0 ]; then echo "tmpfs-test: skipped (needs root; try docker run --privileged)"; exit 0; fi
cd "$(dirname "$0")/.."
exec unshare -m bash -c '
  d=$(mktemp -d); mount -t tmpfs -o size=2m tmpfs "$d"
  BF_TMPFS_DIR="$d/spool" cargo test -p bf-spool --test disk_full -- --ignored --test-threads=1
'
