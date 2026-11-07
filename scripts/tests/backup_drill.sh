#!/usr/bin/env bash
# Integration test for deploy/backup. Needs a Postgres server (DATABASE_URL) and psql/pg_dump
# (PG_BIN for a matching major version). Run: scripts/tests/backup_drill.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
. "$root/deploy/backup/lib.sh"
: "${DATABASE_URL:?}"
t=$(mktemp -d); src=bf_bk_src_$$; dst=bf_bk_dst_$$
cleanup() { for d in $src $dst; do pgx psql "$DATABASE_URL" -qc "DROP DATABASE IF EXISTS $d" >/dev/null 2>&1 || true; done; rm -rf "$t"; }
trap cleanup EXIT
pgx psql "$DATABASE_URL" -qc "CREATE DATABASE $src" >/dev/null
src_url=$(url_with_db "$DATABASE_URL" $src); dst_url=$(url_with_db "$DATABASE_URL" $dst)
for m in "$root"/crates/db/migrations/*.sql; do pgx psql "$src_url" -q -v ON_ERROR_STOP=1 -f "$m" >/dev/null; done
pgx psql "$src_url" -q -v ON_ERROR_STOP=1 >/dev/null <<'SQL'
INSERT INTO workspaces (id, name, slug) VALUES ('00000000-0000-7000-8000-000000000001', 'w', 'w');
INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret)
  VALUES ('00000000-0000-7000-8000-000000000002', '00000000-0000-7000-8000-000000000001', 'p', 'USD', 'pk', 's');
INSERT INTO ledger_entries (id, txn_id, account, program_id, amount_cents, currency)
  SELECT gen_random_uuid(), t, a.acc::ledger_account, '00000000-0000-7000-8000-000000000002', a.amt, 'USD'
  FROM (SELECT gen_random_uuid() AS t FROM generate_series(1, 5)) x,
       LATERAL (VALUES ('cash', 500), ('aff_pending', -500)) AS a(acc, amt);
SQL
# A fake spool: two segments and a checkpoint inside the first.
mkdir -p "$t/spool"; head -c 100 /dev/zero > "$t/spool/00000000000000000000.wal"; head -c 50 /dev/zero > "$t/spool/00000000000000000100.wal"
echo 40 > "$t/spool/checkpoint"

export DATABASE_URL="$src_url" SPOOL_DIR="$t/spool" BF_BACKUP_DIR="$t/bk" BF_BACKUP_KEEP=2
"$root/deploy/backup/backup.sh" >/dev/null
[ ! -e "$t/spool/gc.paused" ] || { echo "FAIL gc left paused"; exit 1; }
dump=$(ls "$t/bk/pg"/*.dump | head -n1); tarf=$(ls "$t/bk/spool"/*.tar | head -n1)
want=$(fingerprint "$src_url")

# backup_checksum_mismatch_aborts_restore
pgx psql "$DATABASE_URL" -qc "CREATE DATABASE $dst" >/dev/null 2>&1 || pgx psql "$(url_with_db "$DATABASE_URL" postgres)" -qc "CREATE DATABASE $dst" >/dev/null
cp "$dump" "$t/bad.dump"; cp "$dump.sha256" "$t/bad.dump.sha256"; printf 'x' >> "$t/bad.dump"
if "$root/deploy/backup/restore.sh" --dump "$t/bad.dump" --target-url "$dst_url" 2>/dev/null; then echo "FAIL backup_checksum_mismatch_aborts_restore"; exit 1; fi
[ "$(pgx psql "$dst_url" -qAt -c "SELECT count(*) FROM pg_tables WHERE schemaname='public'")" = 0 ] || { echo "FAIL bad restore touched target"; exit 1; }

# restore into the empty target works and matches
"$root/deploy/backup/restore.sh" --dump "$dump" --spool-tar "$tarf" --target-url "$dst_url" --spool-target "$t/restored" >/dev/null
[ "$(fingerprint "$dst_url")" = "$want" ] || { echo "FAIL fingerprint differs"; exit 1; }
grep -q 'unbalanced_txns=0' <<<"$want" || { echo "FAIL source unbalanced"; exit 1; }
[ "$(cat "$t/restored/checkpoint")" = 40 ] || { echo "FAIL spool checkpoint"; exit 1; }

# restore_refuses_nonempty_target_without_force
if "$root/deploy/backup/restore.sh" --dump "$dump" --target-url "$dst_url" 2>/dev/null; then echo "FAIL restore_refuses_nonempty_target_without_force"; exit 1; fi
if "$root/deploy/backup/restore.sh" --dump "$dump" --spool-tar "$tarf" --target-url "$(url_with_db "$DATABASE_URL" postgres)" --spool-target "$t/restored" 2>/dev/null; then echo "FAIL nonempty spool target"; exit 1; fi
"$root/deploy/backup/restore.sh" --dump "$dump" --target-url "$dst_url" --force >/dev/null

# retention keeps the newest BF_BACKUP_KEEP
for i in 1 2 3; do sleep 1; "$root/deploy/backup/backup.sh" >/dev/null; done
[ "$(ls "$t/bk/pg"/*.dump | wc -l | tr -d ' ')" = 2 ] || { echo "FAIL retention"; exit 1; }

# the drill itself
"$root/deploy/backup/drill.sh" | tail -n1 | grep -qx PASS || { echo "FAIL drill"; exit 1; }
echo "backup_drill: ok"
