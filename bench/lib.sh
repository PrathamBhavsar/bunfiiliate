#!/usr/bin/env bash
# Helpers for bench/run.sh. Sourced; no side effects at load time.

BF_BENCH_SCHEMA_VERSION=1
BF_BENCH_PINNED_OHA="1.16.0"

# Prints the oha version number (e.g. 1.16.0) of the binary given, or nothing.
bf_oha_version() {
  "$1" --version 2>/dev/null | head -n1 | awk '{print $NF}'
}

# Exit 2 (via return 2) if oha is missing or not the pinned version.
bf_check_oha() {
  local bin="$1" ver
  if ! command -v "$bin" >/dev/null 2>&1; then
    echo "Error: '$bin' not found. Install it yourself, e.g. 'brew install oha' or 'cargo install oha --version $BF_BENCH_PINNED_OHA --locked'. Not installing silently." >&2
    return 2
  fi
  ver="$(bf_oha_version "$bin")"
  if [ "$ver" != "$BF_BENCH_PINNED_OHA" ] && [ "${BF_BENCH_ALLOW_OHA:-0}" != "1" ]; then
    echo "Error: oha $ver found, pinned version is $BF_BENCH_PINNED_OHA. Set BF_BENCH_ALLOW_OHA=1 to override (results will not be comparable)." >&2
    return 2
  fi
  return 0
}

# Strip a trailing "s" so BENCH_DURATION=10 and =10s both work.
bf_seconds() { echo "${1%s}"; }

# Filesystem type of a path (best effort).
bf_fs_type() {
  local p="$1"
  if stat -f %T "$p" >/dev/null 2>&1 && [ "$(uname -s)" = "Darwin" ]; then
    mount | awk -v p="$(df "$p" | awk 'NR==2{print $1}')" '$1==p{gsub(/[(,]/,"",$4); print $4; exit}'
  else
    stat -f -c %T "$p" 2>/dev/null || echo unknown
  fi
}

# bf_hardware_json <spool_dir> <pg_url> <oha_bin> <fsync_modes> <root_dir>
# Prints the hardware JSON object. `class` is $BENCH_CLASS or <os>-<arch>-<cores>c.
bf_hardware_json() {
  local spool="$1" pgurl="$2" oha="$3" modes="$4" root="$5"
  local os arch cores ram cpu osname osver pg rustc ohav sha fs class
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  if [ "$os" = "darwin" ]; then
    cpu="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)"
    cores="$(sysctl -n hw.ncpu 2>/dev/null || echo 0)"
    ram="$(( $(sysctl -n hw.memsize 2>/dev/null || echo 0) / 1024 / 1024 / 1024 ))"
    osname="$(sw_vers -productName 2>/dev/null || echo macOS) $(sw_vers -productVersion 2>/dev/null)"
  else
    cpu="$(awk -F: '/model name/{gsub(/^ /,"",$2); print $2; exit}' /proc/cpuinfo 2>/dev/null || echo unknown)"
    cores="$(nproc 2>/dev/null || echo 0)"
    ram="$(( $(awk '/MemTotal/{print $2}' /proc/meminfo 2>/dev/null || echo 0) / 1024 / 1024 ))"
    osname="$(uname -sr)"
  fi
  pg="$(psql "$pgurl" -t -A -c 'SELECT version();' 2>/dev/null | sed -E 's/, compiled.*//' | head -n1 || true)"
  rustc="$(rustc --version 2>/dev/null || echo unknown)"
  ohav="$("$oha" --version 2>/dev/null | head -n1 || echo unknown)"
  sha="$(git -C "$root" rev-parse --short=12 HEAD 2>/dev/null || echo unknown)"
  fs="$(bf_fs_type "$spool" || true)"
  class="${BENCH_CLASS:-${os}-${arch}-${cores}c}"
  jq -n --arg cpu "$cpu" --argjson cores "${cores:-0}" --argjson ram "${ram:-0}" \
    --arg os "$osname" --arg pg "${pg:-unknown}" --arg rustc "$rustc" --arg oha "$ohav" \
    --arg sha "$sha" --arg modes "$modes" --arg fs "${fs:-unknown}" --arg class "$class" \
    '{cpu:$cpu, cores:$cores, ram_gb:$ram, os:$os, pg_version:$pg, rustc:$rustc,
      oha_version:$oha, git_sha:$sha, fsync_modes:($modes|split(" ")|map(select(length>0))),
      spool_fs:$fs, class:$class}'
}

# bf_summarize_run <oha_json_file>: one compact run object. Errors = transport errors
# plus any response whose status is not 2xx/3xx.
bf_summarize_run() {
  jq -c '
    def ms(k): if (.metrics.latency_ms[k]? != null) then .metrics.latency_ms[k]
               else ((.latencyPercentiles[k] // 0) * 1000 | . * 1000 | round / 1000) end;
    {rps: (.summary.requestsPerSec | tonumber | . * 100 | round / 100),
     p50_ms: ms("p50"), p95_ms: ms("p95"), p99_ms: ms("p99"),
     errors: (([.errorDistribution[]?] | add // 0)
              + ([.statusCodeDistribution // {} | to_entries[]
                  | select((.key | tonumber? // 0) < 200 or (.key | tonumber? // 0) >= 400)
                  | .value] | add // 0)),
     status: (.statusCodeDistribution // {})}' "$1"
}

# bf_assert_no_errors <result_json>: return 1 and say where if any run has errors > 0.
bf_assert_no_errors() {
  local n
  n="$(jq '[.results[].runs[].errors] | add // 0' "$1")"
  if [ "$n" != "0" ]; then
    echo "Error: benchmark recorded $n request errors; failing run." >&2
    return 1
  fi
}

# bf_endpoint_json <endpoint> <fsync> <runs-json-array>: aggregate with median/min/max.
bf_endpoint_json() {
  jq -c -n --arg ep "$1" --arg fs "$2" --argjson runs "$3" '
    def median: sort | if length == 0 then 0
      elif length % 2 == 1 then .[length / 2 | floor]
      else (.[length / 2 - 1] + .[length / 2]) / 2 end;
    ($runs | map(.rps)) as $r | ($runs | map(.p50_ms)) as $p5 | ($runs | map(.p95_ms)) as $p95 | ($runs | map(.p99_ms)) as $p9 |
    {endpoint:$ep, fsync:$fs, runs:$runs,
     median_rps: ($r | median), min_rps: ($r | min), max_rps: ($r | max),
     median_p50_ms: ($p5 | median), median_p95_ms: ($p95 | median), median_p99_ms: ($p9 | median),
     max_p99_ms: ($p9 | max), errors_total: ($runs | map(.errors) | add)}'
}
