#!/usr/bin/env bash
# Tests for check-bench-numbers.sh and docs/benchmarks.md (C42). No Postgres needed.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PASS=0; FAIL=0
ok()  { echo "ok   $1"; PASS=$((PASS+1)); }
bad() { echo "FAIL $1: $2"; FAIL=$((FAIL+1)); }

# check_bench_numbers_fails_on_drift: work on a temp copy of the tree.
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/scripts" "$TMP/site/src" "$TMP/docs"
cp "$ROOT/scripts/check-bench-numbers.sh" "$TMP/scripts/"
cp "$ROOT/site/src/brand.js" "$TMP/site/src/"
cp "$ROOT/docs/benchmarks.md" "$TMP/docs/"
if "$TMP/scripts/check-bench-numbers.sh" >/dev/null 2>&1; then ok check_bench_numbers_passes_on_clean_copy
else bad check_bench_numbers_passes_on_clean_copy "clean copy failed"; fi
sed -i.bak 's/27\.6k req\/s/99.9k req\/s/' "$TMP/site/src/brand.js"
if "$TMP/scripts/check-bench-numbers.sh" >/dev/null 2>&1; then bad check_bench_numbers_fails_on_drift "drift not detected"
else ok check_bench_numbers_fails_on_drift; fi

# docs_contains_hardware_block
d="$ROOT/docs/benchmarks.md"; miss=""
for k in "Test Environment" "**CPU**" "BF_FSYNC" "Single-Machine Localhost Run" "floor" "Publishing Rules" "bench/run.sh --write-docs"; do
  grep -qF -- "$k" "$d" || miss="$miss [$k]"
done
if [ -z "$miss" ]; then ok docs_contains_hardware_block; else bad docs_contains_hardware_block "missing:$miss"; fi

echo "passed=$PASS failed=$FAIL"
[ "$FAIL" = 0 ]
