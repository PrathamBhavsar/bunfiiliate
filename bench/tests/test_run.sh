#!/usr/bin/env bash
# Shell tests for bench/run.sh helpers. Usage: bench/tests/test_run.sh
# quick_run_writes_valid_json needs Postgres + a release build; set BENCH_TEST_SKIP_QUICK=1 to skip it.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
. "$ROOT/bench/lib.sh"
PASS=0; FAIL=0
ok()   { echo "ok   $1"; PASS=$((PASS+1)); }
bad()  { echo "FAIL $1: $2"; FAIL=$((FAIL+1)); }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

# --- missing_oha_exits_2
BF_BENCH_OHA_BIN=oha-does-not-exist "$ROOT/bench/run.sh" --quick >/dev/null 2>"$TMP/err"; rc=$?
if [ "$rc" = 2 ] && grep -q "Not installing silently" "$TMP/err"; then ok missing_oha_exits_2; else bad missing_oha_exits_2 "rc=$rc"; fi

# --- wrong oha version exits 2 unless allowed
printf '#!/bin/sh\necho "oha 9.9.9"\n' > "$TMP/oha"; chmod +x "$TMP/oha"
BF_BENCH_OHA_BIN="$TMP/oha" "$ROOT/bench/run.sh" --quick >/dev/null 2>&1; rc=$?
if [ "$rc" = 2 ]; then ok wrong_oha_version_exits_2; else bad wrong_oha_version_exits_2 "rc=$rc"; fi
BF_BENCH_ALLOW_OHA=1 bf_check_oha "$TMP/oha" && ok allow_oha_override || bad allow_oha_override "rejected"

# --- hardware_json_has_required_keys
mkdir -p "$TMP/spool"
hw="$(bf_hardware_json "$TMP/spool" "postgres://invalid/none" "$TMP/oha" "always off" "$ROOT")"
missing="$(jq -r '["cpu","cores","ram_gb","os","pg_version","rustc","oha_version","git_sha","fsync_modes","spool_fs","class"] - keys | join(",")' <<< "$hw")"
if [ -z "$missing" ] && jq -e '.cores > 0 and (.git_sha|length > 0) and (.fsync_modes|length == 2)' <<< "$hw" >/dev/null; then
  ok hardware_json_has_required_keys; else bad hardware_json_has_required_keys "missing: $missing"; fi

# --- error_rate_nonzero_fails_run
mk() { jq -n --argjson e "$1" '{results:[{runs:[{errors:$e}]}]}' > "$TMP/r.json"; }
mk 0; bf_assert_no_errors "$TMP/r.json" 2>/dev/null && ok zero_errors_passes || bad zero_errors_passes "failed"
mk 3; if bf_assert_no_errors "$TMP/r.json" 2>/dev/null; then bad error_rate_nonzero_fails_run "passed"; else ok error_rate_nonzero_fails_run; fi

# --- summarize counts non-2xx/3xx statuses and transport errors as errors
echo '{"summary":{"requestsPerSec":100.123},"latencyPercentiles":{"p50":0.001,"p95":0.002,"p99":0.003},"statusCodeDistribution":{"204":90,"503":5},"errorDistribution":{"timeout":2}}' > "$TMP/oha.json"
e="$(bf_summarize_run "$TMP/oha.json" | jq .errors)"
[ "$e" = 7 ] && ok summarize_counts_bad_status_and_transport || bad summarize_counts_bad_status_and_transport "errors=$e"
p99="$(bf_summarize_run "$TMP/oha.json" | jq .p99_ms)"
[ "$p99" = 3 ] && ok summarize_converts_seconds_to_ms || bad summarize_converts_seconds_to_ms "p99=$p99"

# --- endpoint aggregation median/min/max
agg="$(bf_endpoint_json /c always '[{"rps":10,"p50_ms":1,"p95_ms":2,"p99_ms":3,"errors":0,"status":{}},{"rps":30,"p50_ms":1,"p95_ms":2,"p99_ms":9,"errors":0,"status":{}},{"rps":20,"p50_ms":1,"p95_ms":2,"p99_ms":5,"errors":0,"status":{}}]')"
[ "$(jq -c '[.median_rps,.min_rps,.max_rps,.median_p99_ms]' <<< "$agg")" = "[20,10,30,5]" ] && ok aggregate_median_min_max || bad aggregate_median_min_max "$agg"

# --- quick_run_writes_valid_json (real run)
if [ "${BENCH_TEST_SKIP_QUICK:-0}" = 1 ]; then
  echo "skip quick_run_writes_valid_json (BENCH_TEST_SKIP_QUICK=1)"
else
  if "$ROOT/bench/run.sh" --quick >"$TMP/quick.log" 2>&1; then
    f="$(ls -t "$ROOT"/bench/results/*-quick.json | head -n1)"
    if jq -e '.schema_version == 1 and .quick == true and (.hardware.class|length>0) and (.results|length == 6)
              and all(.results[]; (.runs|length) >= 1 and .errors_total == 0 and .median_rps > 0)' "$f" >/dev/null; then
      ok quick_run_writes_valid_json; else bad quick_run_writes_valid_json "schema check failed on $f"; fi
  else
    bad quick_run_writes_valid_json "run.sh --quick failed; see tail: $(tail -n3 "$TMP/quick.log")"
  fi
fi

echo "passed=$PASS failed=$FAIL"
[ "$FAIL" = 0 ]
