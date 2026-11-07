#!/bin/sh
# Lists every sqlx::query* call site in db and api sources into docs/audit/queries.txt.
# Format: file:line: call. Re-run after touching SQL; CI-free, reviewed by humans (see C03 for the lint).
set -eu
cd "$(dirname "$0")/.."
grep -rnE 'sqlx::query(_as|_scalar)?(::<[^>]*>)?!?\(|query(_as|_scalar)?::<' crates/db/src crates/api/src \
  | sed -E 's/^([^:]+:[0-9]+):[[:space:]]*/\1: /' | LC_ALL=C sort > docs/audit/queries.txt
wc -l < docs/audit/queries.txt
