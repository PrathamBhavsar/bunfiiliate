# shellcheck shell=bash
# Shared helpers for backup.sh / restore.sh / drill.sh.
# Set PG_BIN to a directory holding pg_dump/pg_restore/psql when the default ones are not
# the same major version as the server (pg_dump must be >= the server).
pgx() { local t=$1; shift; if [ -n "${PG_BIN:-}" ]; then "$PG_BIN/$t" "$@"; else "$t" "$@"; fi; }
die() { echo "error: $*" >&2; exit 1; }
sha_of() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
# Replace the database name in a postgres:// URL.
url_with_db() { printf '%s' "$1" | sed -E "s#(://[^/]*/)[^?]*#\\1$2#"; }
# Row counts + ledger fingerprint of a database; stdout is compared by drill.sh.
fingerprint() {
  pgx psql "$1" -qAt -v ON_ERROR_STOP=1 <<'SQL'
SELECT 'affiliates=' || count(*) FROM affiliates;
SELECT 'conversions=' || count(*) FROM conversions;
SELECT 'payouts=' || count(*) FROM payouts;
SELECT 'ledger_entries=' || count(*) FROM ledger_entries;
SELECT 'ledger_hash=' || coalesce(md5(string_agg(id::text || ':' || txn_id::text || ':' || account::text || ':' || amount_cents::text, ',' ORDER BY id)), '-') FROM ledger_entries;
SELECT 'unbalanced_txns=' || count(*) FROM (SELECT txn_id FROM ledger_entries GROUP BY txn_id HAVING sum(amount_cents) <> 0) x;
SQL
}
