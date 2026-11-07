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
mkdir -p "$TMP/scripts" "$TMP/site" "$TMP/bench"
cp "$ROOT/scripts/check-bench-numbers.sh" "$TMP/scripts/"
cp "$ROOT/site/index.html" "$TMP/site/"
cp -R "$ROOT/bench/results" "$ROOT/bench/gen_docs.py" "$TMP/bench/"
if "$TMP/scripts/check-bench-numbers.sh" >/dev/null 2>&1; then ok check_bench_numbers_passes_on_clean_copy
else bad check_bench_numbers_passes_on_clean_copy "clean copy failed"; fi
python3 -I - "$TMP/site/index.html" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p, encoding="utf-8").read()
s2 = re.sub(r'(data-metric="rps">)([0-9.]+)', lambda m: m.group(1) + str(float(m.group(2)) + 1), s, count=1)
assert s != s2
open(p, "w", encoding="utf-8").write(s2)
PY
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
