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

# gen_docs --write-docs renders the docs table and the site facts from a full result; no site/index.html.
W="$(mktemp -d)"; trap 'rm -rf "$TMP" "$W"' EXIT
mkdir -p "$W/bench" "$W/site/src" "$W/scripts" "$W/docs"
cp "$ROOT/bench/gen_docs.py" "$W/bench/"
cp "$ROOT/site/src/brand.js" "$W/site/src/"
cp "$ROOT/scripts/check-bench-numbers.sh" "$W/scripts/"
python3 - "$W/result.json" <<'PY'
import json, sys

results = []
for fs, rps_v, p50 in (("always", 1500.0, 13.4), ("off", 27700.0, 0.42)):
    for ep in ("/c", "/r/:code", "/v1/events/:key"):
        results.append({
            "endpoint": ep, "fsync": fs,
            "runs": [{"rps": rps_v, "p50_ms": p50, "p95_ms": p50 * 2, "p99_ms": p50 * 3,
                      "errors": 0, "status": {"204": 1}}],
            "median_rps": rps_v, "min_rps": rps_v, "max_rps": rps_v,
            "median_p50_ms": p50, "median_p95_ms": p50 * 2, "median_p99_ms": p50 * 3,
            "max_p99_ms": p50 * 3, "errors_total": 0,
        })
doc = {
    "schema_version": 1, "date": "2026-01-01", "git_sha": "abc1234", "quick": False,
    "hardware": {"cpu": "test", "cores": 4, "ram_gb": 8, "os": "test", "pg_version": "16",
                 "rustc": "1.99", "oha_version": "1.16.0", "git_sha": "abc1234",
                 "fsync_modes": ["always", "off"], "spool_fs": "ext4", "class": "test-4c"},
    "params": {"duration_s": 10, "repeats": 5, "concurrency": 20, "warmup_s": 3, "seed": 42},
    "results": results,
}
json.dump(doc, open(sys.argv[1], "w"))
PY
if python3 -I "$W/bench/gen_docs.py" "$W/result.json" >/dev/null 2>&1 \
  && grep -q '| `/r/:code` | off | 27700 (27700 to 27700)' "$W/docs/benchmarks.md" \
  && grep -q '\["Tracker p50, fsync off", "0.4 ms · 27.7k req/s"\]' "$W/site/src/brand.js"; then
  ok write_docs_renders_docs_and_site_facts
else bad write_docs_renders_docs_and_site_facts "gen_docs did not update docs + brand.js"; fi
if [ -e "$W/site/index.html" ]; then bad write_docs_has_no_site_index "gen_docs created site/index.html"
else ok write_docs_has_no_site_index; fi
if "$W/scripts/check-bench-numbers.sh" >/dev/null 2>&1; then ok check_bench_numbers_passes_after_write_docs
else bad check_bench_numbers_passes_after_write_docs "gate failed after gen_docs"; fi
if python3 -I "$W/bench/gen_docs.py" --check "$W/result.json" >/dev/null 2>&1; then ok check_accepts_fresh_output
else bad check_accepts_fresh_output "gen_docs --check rejected its own output"; fi
sed -i.bak 's/27\.7k req\/s/99.9k req\/s/' "$W/site/src/brand.js"
if python3 -I "$W/bench/gen_docs.py" --check "$W/result.json" >/dev/null 2>&1; then bad check_brand_fails_on_drift "drift not detected"
else ok check_brand_fails_on_drift; fi

echo "passed=$PASS failed=$FAIL"
[ "$FAIL" = 0 ]
