#!/usr/bin/env bash
# Usage: scripts/snapshot-schema.sh <N>
# Applies migrations 0001..N (by file prefix) to a scratch database, seeds minimal rows,
# records them in _sqlx_migrations, and writes crates/db/tests/snapshots/<N>.sql
# (schema + data, INSERT form). Needs psql/pg_dump on PATH and ADMIN_URL (default
# $DATABASE_URL) pointing at a server where we may create databases.
set -euo pipefail
n=${1:?usage: snapshot-schema.sh <N>}
root=$(cd "$(dirname "$0")/.." && pwd)
admin=${ADMIN_URL:-${DATABASE_URL:?set DATABASE_URL}}
pgbin=${PGBIN:-/opt/homebrew/opt/postgresql@16/bin}
[ -x "$pgbin/pg_dump" ] && export PATH="$pgbin:$PATH"
db="bf_snap_$$"
base=${admin%/*}
url="$base/$db"
psql "$admin" -qc "CREATE DATABASE $db"
trap 'psql "$admin" -qc "DROP DATABASE IF EXISTS $db" >/dev/null' EXIT
psql "$url" -q -v ON_ERROR_STOP=1 <<'SQL'
CREATE TABLE _sqlx_migrations (
    version BIGINT PRIMARY KEY, description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(), success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL, execution_time BIGINT NOT NULL);
SQL
for f in "$root"/crates/db/migrations/*.sql; do
  b=$(basename "$f" .sql); v=$((10#${b%%_*})); [ "$v" -le "$((10#$n))" ] || continue
  desc=${b#*_}; desc=${desc//_/ }
  psql "$url" -q -1 -v ON_ERROR_STOP=1 -f "$f"
  sum=$(openssl dgst -sha384 -binary "$f" | xxd -p | tr -d '\n')
  psql "$url" -q -v ON_ERROR_STOP=1 -c "INSERT INTO _sqlx_migrations VALUES ($v, '$desc', '2026-01-01T00:00:00Z', true, decode('$sum','hex'), 0)"
  if [ "$v" -eq 1 ]; then
    psql "$url" -q -v ON_ERROR_STOP=1 <<'SQL'
INSERT INTO workspaces (id, name, slug) VALUES ('00000000-0000-0000-0000-0000000000a1', 'Seed WS', 'seed-ws');
INSERT INTO users (id, workspace_id, email, password_hash, role)
  VALUES ('00000000-0000-0000-0000-0000000000b1', '00000000-0000-0000-0000-0000000000a1', 'seed@example.com', 'x', 'admin');
SQL
  fi
done
out="$root/crates/db/tests/snapshots/$(printf '%04d' $((10#$n))).sql"
# \restrict lines carry a random token; drop them so the file is deterministic.
pg_dump "$url" --no-owner --no-privileges --column-inserts | grep -vE '^\\(un)?restrict ' > "$out"
echo "wrote $out"
