# shellcheck shell=bash
# Shared helpers for backup.sh / restore.sh / drill.sh.
# Set PG_BIN to a directory holding pg_dump/pg_restore/psql when the default ones are not
# the same major version as the server (pg_dump must be >= the server).
# A postgres:// argument has its password moved to PGPASSWORD so it never shows in `ps` (the
# password must be URL-safe, see self-hosting.md; it is not percent-decoded).
pgx() {
  local t=$1; shift; local bin=$t a=() x pw=""
  [ -z "${PG_BIN:-}" ] || bin="$PG_BIN/$t"
  for x in "$@"; do
    case $x in
      postgres://*|postgresql://*)
        pw=$(printf '%s' "$x" | sed -nE 's#^[a-z]+://[^:/@]+:([^@]*)@.*#\1#p')
        x=$(printf '%s' "$x" | sed -E 's#^([a-z]+://[^:/@]+):[^@]*@#\1@#');;
    esac
    a+=("$x")
  done
  if [ -n "$pw" ]; then PGPASSWORD=$pw "$bin" "${a[@]}"; else "$bin" "${a[@]}"; fi
}
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
  # Per-partition click counts (C24): a restore that loses a partition or lands rows in the wrong
  # one differs here even when the grand total matches. Sorted so output is stable.
  pgx psql "$1" -qAt -v ON_ERROR_STOP=1 <<'SQL'
SELECT 'clicks_total=' || count(*) FROM clicks;
SELECT 'clicks:' || tableoid::regclass::text || '=' || count(*) FROM clicks GROUP BY tableoid ORDER BY 1;
SQL
}
