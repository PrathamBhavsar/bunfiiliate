#!/usr/bin/env bash
# Migration policy check (docs/ops/migrations.md).
#  1. names are NNNN_name.sql, prefixes unique (gaps allowed)
#  2. no existing migration is modified/deleted vs $BASE_REF (default origin/staging, else staging)
#  3. DROP TABLE / DROP COLUMN needs an `-- allow-destructive: <reason>` line in the file
# MIGRATIONS_DIR overrides the directory (used by scripts/tests/check_migrations_test.sh).
set -euo pipefail
dir=${MIGRATIONS_DIR:-crates/db/migrations}
fail=0
err() { echo "check-migrations: $*" >&2; fail=1; }

prev=""
for f in "$dir"/*.sql; do
  b=$(basename "$f")
  [[ $b =~ ^([0-9]{4})_[a-z0-9_]+\.sql$ ]] || { err "bad name: $b"; continue; }
  p=${BASH_REMATCH[1]}
  [ "$p" != "$prev" ] || err "duplicate prefix $p"
  [[ -z $prev || $p > $prev ]] || err "not increasing: $b"
  prev=$p
  if grep -Eiq '^[^-]*\bdrop[[:space:]]+(table|column)\b' "$f" && ! grep -q -- '-- allow-destructive: .\+' "$f"; then
    err "$b: DROP TABLE/COLUMN without '-- allow-destructive: <reason>'"
  fi
done

base=${BASE_REF:-}
if [ -z "$base" ]; then
  for r in origin/staging staging; do git rev-parse -q --verify "$r" >/dev/null && { base=$r; break; }; done
fi
if [ -n "$base" ]; then
  # A = added is fine; anything else (M, D, R, T) touching an existing file is not.
  while read -r st path _; do
    [ -n "$st" ] && [ "$st" != A ] && err "existing migration changed ($st): $path"
  done < <(git diff --name-status --no-renames "$base"...HEAD -- "$dir"; git diff --name-status --no-renames HEAD -- "$dir")
else
  echo "check-migrations: no base ref found; skipped history check" >&2
fi
[ "$fail" -eq 0 ] && echo "check-migrations: ok"
exit "$fail"
