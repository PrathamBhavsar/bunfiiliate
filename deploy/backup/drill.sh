#!/usr/bin/env bash
# Restore drill: back up the live DB + spool, restore the newest backup into a scratch DB and
# temp spool dir, compare row counts and the ledger fingerprint with the source. Exit 0 = PASS.
# Env: DATABASE_URL, SPOOL_DIR (required), BF_BACKUP_DIR (default: temp), PG_BIN.
# Not covered here: tracker replay of the restored spool (see crates/spool/tests/restore_replay.rs
# for the idempotency check; the tracker replays at start-up).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"
: "${DATABASE_URL:?}" "${SPOOL_DIR:?}"

work=$(mktemp -d); scratch=bf_drill_$$_$(date +%s)
export BF_BACKUP_DIR=${BF_BACKUP_DIR:-$work/backups}
scratch_url=$(url_with_db "$DATABASE_URL" "$scratch")
cleanup() { pgx psql "$DATABASE_URL" -qc "DROP DATABASE IF EXISTS \"$scratch\"" >/dev/null 2>&1 || true; rm -rf "$work"; }
trap cleanup EXIT

"$here/backup.sh"
dump=$(ls -1 "$BF_BACKUP_DIR"/pg/*.dump | sort | tail -n 1)
tarf=$(ls -1 "$BF_BACKUP_DIR"/spool/*.tar | sort | tail -n 1)

pgx psql "$DATABASE_URL" -qc "CREATE DATABASE \"$scratch\"" >/dev/null
"$here/restore.sh" --dump "$dump" --spool-tar "$tarf" --target-url "$scratch_url" --spool-target "$work/spool"

# Restored checkpoint must not be past the end of the copied segments (else records are lost).
if [ -f "$work/spool/checkpoint" ]; then
  cp_off=$(tr -d '[:space:]' < "$work/spool/checkpoint")
  end=0
  for f in "$work/spool"/*.wal; do
    [ -e "$f" ] || continue
    start=$((10#$(basename "$f" .wal))); size=$(wc -c < "$f"); [ $((start + size)) -gt "$end" ] && end=$((start + size))
  done
  [ "$cp_off" -le "$end" ] || { echo "FAIL: restored checkpoint $cp_off is past the last segment end $end"; exit 1; }
fi

a=$(fingerprint "$DATABASE_URL"); b=$(fingerprint "$scratch_url")
if [ "$a" = "$b" ] && ! grep -q 'unbalanced_txns=[1-9]' <<<"$b"; then
  echo "$b"; echo "PASS"
else
  echo "--- source"; echo "$a"; echo "--- restored"; echo "$b"; echo "FAIL"; exit 1
fi
