#!/usr/bin/env bash
# Nightly backup: Postgres logical dump + spool copy. See docs/ops/backup-restore.md.
#
# Order matters (REVISIONS C10):
#   1. pause spool GC (flag file) so no segment disappears mid-copy
#   2. copy the checkpoint file, THEN the segments
#   3. pg_dump (the DB may be ahead of the copied checkpoint: replay is idempotent)
#   4. resume GC (also on failure, via trap)
#
# Env: DATABASE_URL, SPOOL_DIR, BF_BACKUP_DIR (required); BF_BACKUP_KEEP (14);
#      BF_BACKUP_REMOTE (optional rclone/rsync destination); PG_BIN.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

: "${DATABASE_URL:?}" "${SPOOL_DIR:?}" "${BF_BACKUP_DIR:?}"
keep=${BF_BACKUP_KEEP:-14}
ts=$(date -u +%Y%m%dT%H%M%SZ)
out=$BF_BACKUP_DIR
mkdir -p "$out/pg" "$out/spool"
stage=$(mktemp -d "$out/.stage.XXXXXX")

resume() { rm -f "$SPOOL_DIR/gc.paused"; rm -rf "$stage"; }
trap resume EXIT
trap 'exit 143' INT TERM HUP

mkdir -p "$SPOOL_DIR"
# The flag holds our pid. One left by a SIGKILLed backup (pid gone) would stop spool GC for good:
# clear it. A live pid means another backup is running.
if [ -f "$SPOOL_DIR/gc.paused" ]; then
  old=$(cat "$SPOOL_DIR/gc.paused" 2>/dev/null || true)
  if [ -n "$old" ] && kill -0 "$old" 2>/dev/null; then die "another backup is running (pid $old)"; fi
  echo "removing stale $SPOOL_DIR/gc.paused (pid ${old:-unknown} is gone)" >&2
fi
echo $$ > "$SPOOL_DIR/gc.paused"                             # 1. pause GC
if [ -f "$SPOOL_DIR/checkpoint" ]; then cp "$SPOOL_DIR/checkpoint" "$stage/checkpoint"; fi   # 2a
find "$SPOOL_DIR" -maxdepth 1 -type f -name '*.wal' -exec cp {} "$stage/" \;                  # 2b
tar -C "$stage" -cf "$out/spool/$ts.tar" .
sha_of "$out/spool/$ts.tar" > "$out/spool/$ts.tar.sha256"

# 3. Dump to a name no `*.dump` glob matches, rename once complete (a killed dump leaves no
# half file for restore or the drill to pick up).
pgx pg_dump -Fc --no-owner -f "$out/pg/$ts.dump.partial" "$DATABASE_URL"
mv "$out/pg/$ts.dump.partial" "$out/pg/$ts.dump"
sha_of "$out/pg/$ts.dump" > "$out/pg/$ts.dump.sha256"
resume; trap - EXIT INT TERM HUP                                                               # 4

# Keep the newest $keep of each kind.
prune() { ls -1 "$out/$1"/*."$2" 2>/dev/null | sort -r | tail -n +"$((keep + 1))" | while read -r f; do rm -f "$f" "$f.sha256"; done; }
prune pg dump
prune spool tar

if [ -n "${BF_BACKUP_REMOTE:-}" ]; then
  if command -v rclone >/dev/null; then rclone copy "$out" "$BF_BACKUP_REMOTE"
  else rsync -a "$out/" "$BF_BACKUP_REMOTE"; fi
fi
echo "backup $ts ok: $out/pg/$ts.dump $out/spool/$ts.tar"
