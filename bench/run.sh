#!/usr/bin/env bash
# Reproducible tracker load benchmark. See bench/README.md.
#   bench/run.sh [--quick] [--write-docs]
# Env: DATABASE_URL (default postgres://$USER@localhost:5433/postgres), BENCH_DURATION (s, 10),
#      BENCH_REPEATS (5), BENCH_CONC (20), BENCH_WARMUP (s, 3), BENCH_SEED (42),
#      BENCH_SETTLE (s between runs, 5), BENCH_CLASS, BF_BENCH_OHA_BIN (oha), BF_BENCH_ALLOW_OHA, BENCH_SPOOL_ROOT.
set -euo pipefail

export PATH="/opt/homebrew/opt/postgresql@16/bin:/opt/homebrew/bin:/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=bench/lib.sh
. "$SCRIPT_DIR/lib.sh"

QUICK=0
WRITE_DOCS=0
for arg in "$@"; do
  case "$arg" in
    --quick) QUICK=1 ;;
    --write-docs) WRITE_DOCS=1 ;;
    *) echo "usage: bench/run.sh [--quick] [--write-docs]" >&2; exit 64 ;;
  esac
done

if [ "$QUICK" = 1 ]; then
  BENCH_DURATION="${BENCH_DURATION:-2}"
  BENCH_REPEATS="${BENCH_REPEATS:-1}"
  BENCH_WARMUP="${BENCH_WARMUP:-1}"
fi
DURATION="$(bf_seconds "${BENCH_DURATION:-10}")"
REPEATS="${BENCH_REPEATS:-5}"
CONC="${BENCH_CONC:-20}"
WARMUP="$(bf_seconds "${BENCH_WARMUP:-3}")"
SEED="${BENCH_SEED:-42}"
SETTLE="${BENCH_SETTLE:-5}"
OHA="${BF_BENCH_OHA_BIN:-oha}"

# Preflight (before any build): oha present and pinned.
bf_check_oha "$OHA" || exit 2
for cmd in cargo psql openssl jq curl python3; do
  command -v "$cmd" >/dev/null 2>&1 || { echo "Error: required command '$cmd' not on PATH." >&2; exit 1; }
done

echo "=== Building release binaries ==="
cargo build --release -p api -p tracker --manifest-path "$ROOT_DIR/Cargo.toml"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
API_BIN="$TARGET_DIR/release/api"
TRACKER_BIN="$TARGET_DIR/release/tracker"

BASE_URL="${DATABASE_URL:-postgres://${USER:-postgres}@localhost:5433/postgres}"
SCRATCH_DB_NAME="bf_bench"
SCRATCH_URL="${BASE_URL%/*}/$SCRATCH_DB_NAME"
SPOOL_ROOT="${BENCH_SPOOL_ROOT:-${TMPDIR:-/tmp}/bf_bench_spool}"
mkdir -p "$SPOOL_ROOT"

TRACKER_PID=""
CURRENT_SPOOL_DIR=""
RAW_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bf_bench_raw_XXXXXX")"

cleanup() {
  local exit_code=$?
  if [ -n "$TRACKER_PID" ] && kill -0 "$TRACKER_PID" 2>/dev/null; then
    kill -INT "$TRACKER_PID" 2>/dev/null || true
    sleep 1
    kill -9 "$TRACKER_PID" 2>/dev/null || true
    wait "$TRACKER_PID" 2>/dev/null || true
  fi
  if [ -n "$CURRENT_SPOOL_DIR" ] && [ -d "$CURRENT_SPOOL_DIR" ]; then rm -rf "$CURRENT_SPOOL_DIR"; fi
  rm -rf "$RAW_DIR"
  psql "$BASE_URL" -c "DROP DATABASE IF EXISTS $SCRATCH_DB_NAME;" >/dev/null 2>&1 || true
  exit "$exit_code"
}
trap cleanup EXIT INT TERM

echo "=== Creating scratch database ($SCRATCH_DB_NAME) ==="
psql "$BASE_URL" -q -c "DROP DATABASE IF EXISTS $SCRATCH_DB_NAME;"
psql "$BASE_URL" -q -c "CREATE DATABASE $SCRATCH_DB_NAME;"

INIT_SPOOL="$(mktemp -d "$SPOOL_ROOT/init_XXXXXX")"
CURRENT_SPOOL_DIR="$INIT_SPOOL"
DATABASE_URL="$SCRATCH_URL" API_PORT=8080 TRACKER_PORT=8081 \
JWT_SECRET="bench-secret-bench-secret-bench-secret" SPOOL_DIR="$INIT_SPOOL" \
"$API_BIN" bootstrap --email "admin@example.com" --password "supersecret123" --workspace "Bench Workspace"
rm -rf "$INIT_SPOOL"
CURRENT_SPOOL_DIR=""

# Seeded code set: NCODES codes derived from BENCH_SEED (same seed, same codes).
NCODES=16
CODES=()
while IFS= read -r c; do CODES+=("$c"); done < <(
  awk -v seed="$SEED" -v n="$NCODES" 'BEGIN{srand(seed); for(i=0;i<n;i++) printf "bc%08x\n", int(rand()*4294967295)}')
CODES_SQL="$(printf "'%s'," "${CODES[@]}")"; CODES_SQL="${CODES_SQL%,}"

echo "=== Seeding program, affiliate and ${NCODES} referral codes (seed $SEED) ==="
psql "$SCRATCH_URL" -q << SEED_SQL
DO \$\$
DECLARE ws_id uuid; aff_user_id uuid; aff_id uuid; prog_id uuid; c text;
BEGIN
    SELECT id INTO ws_id FROM workspaces LIMIT 1;
    INSERT INTO users (id, workspace_id, email, password_hash, role)
    VALUES (gen_random_uuid(), ws_id, 'affiliate@example.com', 'hash', 'affiliate')
    RETURNING id INTO aff_user_id;
    INSERT INTO affiliates (id, workspace_id, user_id, status, payout_email)
    VALUES (gen_random_uuid(), ws_id, aff_user_id, 'active', 'affiliate@example.com')
    RETURNING id INTO aff_id;
    INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret)
    VALUES (gen_random_uuid(), ws_id, 'Bench Program', 'USD', 'pk_bench', 'whsec_bench_secret')
    RETURNING id INTO prog_id;
    INSERT INTO enrollments (affiliate_id, program_id) VALUES (aff_id, prog_id);
    FOREACH c IN ARRAY ARRAY[$CODES_SQL] LOOP
        INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url)
        VALUES (c, aff_id, prog_id, 'https://example.com/target');
    END LOOP;
END \$\$;
SEED_SQL

WEBHOOK_SECRET="whsec_bench_secret"
WEBHOOK_BODY='{"event":"conversion","customer_id":"cust_bench","amount":1000}'
WEBHOOK_KEY="bench-idemp-1"
BASE="http://127.0.0.1:8081"

# run_oha <out> <seconds> <oha args...>
run_oha() {
  local out="$1" dur="$2"; shift 2
  sleep "$SETTLE"  # let the tracker writer queue and Postgres ingest drain between runs
  "$OHA" -z "${dur}s" -c "$CONC" -w --no-tui --output-format json "$@" > "$out"
}

# measure <endpoint> <fsync>: warm-up (discarded) then REPEATS runs; echoes endpoint JSON.
measure() {
  local ep="$1" fsync="$2" i code ts sig body runs="[]" f args
  for i in $(seq 0 "$REPEATS"); do
    code="${CODES[$(( (SEED + i * 7) % NCODES ))]}"
    case "$ep" in
      /c)
        body="{\"code\":\"$code\",\"vid\":\"vid_bench\",\"url\":\"https://example.com/target\",\"ref\":\"\"}"
        args=(-m POST -H "Content-Type: application/json" -d "$body" "$BASE/c") ;;
      /r/:code)
        args=("$BASE/r/$code") ;;
      /v1/events/:key)
        # Signature covers "<ts>.<key>.<body>"; fresh timestamp per run (5 min tolerance).
        ts=$(date +%s)
        sig=$(printf "%s.%s.%s" "$ts" "$WEBHOOK_KEY" "$WEBHOOK_BODY" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" | awk '{print $NF}')
        args=(-m POST -H "Content-Type: application/json" -H "Idempotency-Key: $WEBHOOK_KEY"
              -H "X-Bf-Timestamp: $ts" -H "X-Bf-Signature: $sig" -d "$WEBHOOK_BODY" "$BASE/v1/events/pk_bench") ;;
    esac
    f="$RAW_DIR/run.json"
    if [ "$i" = 0 ]; then
      if [ "$WARMUP" -gt 0 ]; then run_oha "$f" "$WARMUP" "${args[@]}"; fi
    else
      run_oha "$f" "$DURATION" "${args[@]}"
      runs="$(jq -c --argjson r "$(bf_summarize_run "$f")" '. + [$r]' <<< "$runs")"
    fi
  done
  bf_endpoint_json "$ep" "$fsync" "$runs"
}

RESULT_ITEMS=()
SPOOL_FS_DIR=""
for FSYNC_MODE in always off; do
  echo "--- BF_FSYNC=$FSYNC_MODE ---"
  SPOOL_DIR="$(mktemp -d "$SPOOL_ROOT/${FSYNC_MODE}_XXXXXX")"
  CURRENT_SPOOL_DIR="$SPOOL_DIR"

  # The bench is not about disk pressure (crates/spool/tests/disk_full.rs is).
  DATABASE_URL="$SCRATCH_URL" API_PORT=8080 TRACKER_PORT=8081 \
  JWT_SECRET="bench-secret-bench-secret-bench-secret" \
  BF_IP_SALT="bench-ip-salt-bench-ip-salt" \
  BF_MASTER_KEYS="bench1:YmVuY2gtbWFzdGVyLWtleS1iZW5jaC1tYXN0ZXItayE=" \
  BF_SPOOL_HIGH_WATER_PCT=99 BF_SPOOL_LOW_WATER_PCT=98 BF_SPOOL_MIN_FREE_BYTES=1048576 \
  SPOOL_DIR="$SPOOL_DIR" BF_FSYNC="$FSYNC_MODE" \
  "$TRACKER_BIN" &
  TRACKER_PID=$!

  for _ in $(seq 1 50); do
    curl -s "$BASE/healthz" >/dev/null 2>&1 && break
    sleep 0.1
  done
  curl -s "$BASE/healthz" >/dev/null 2>&1 || { echo "Error: tracker failed to start (BF_FSYNC=$FSYNC_MODE)" >&2; exit 1; }

  # Record the spool fs once, while a spool dir exists.
  if [ -z "$SPOOL_FS_DIR" ]; then HW="$(bf_hardware_json "$SPOOL_DIR" "$BASE_URL" "$OHA" "always off" "$ROOT_DIR")"; SPOOL_FS_DIR=1; fi

  for ep in /c /r/:code /v1/events/:key; do
    echo "Benchmarking $ep (BF_FSYNC=$FSYNC_MODE, ${REPEATS}x${DURATION}s, warm-up ${WARMUP}s)"
    RESULT_ITEMS+=("$(measure "$ep" "$FSYNC_MODE")")
  done

  kill -INT "$TRACKER_PID" 2>/dev/null || true
  wait "$TRACKER_PID" 2>/dev/null || true
  TRACKER_PID=""
  rm -rf "$SPOOL_DIR"
  CURRENT_SPOOL_DIR=""
done

SHA="$(jq -r .git_sha <<< "$HW")"
DATE="$(date -u +%Y-%m-%d)"
SUFFIX=""; if [ "$QUICK" = 1 ]; then SUFFIX="-quick"; fi
OUT="$SCRIPT_DIR/results/${DATE}-${SHA}${SUFFIX}.json"
mkdir -p "$SCRIPT_DIR/results"

printf '%s\n' "${RESULT_ITEMS[@]}" | jq -s \
  --argjson hw "$HW" --argjson sv "$BF_BENCH_SCHEMA_VERSION" --arg date "$DATE" \
  --argjson quick "$([ "$QUICK" = 1 ] && echo true || echo false)" \
  --argjson dur "$DURATION" --argjson rep "$REPEATS" --argjson conc "$CONC" \
  --argjson warm "$WARMUP" --argjson seed "$SEED" '
  {schema_version:$sv, date:$date, git_sha:$hw.git_sha, quick:$quick, hardware:$hw,
   params:{duration_s:$dur, repeats:$rep, concurrency:$conc, warmup_s:$warm, seed:$seed},
   results:.}' > "$OUT"

echo "Results: $OUT"
bf_assert_no_errors "$OUT" || exit 1

if [ "$WRITE_DOCS" = 1 ]; then
  if [ "$QUICK" = 1 ]; then
    echo "Error: --write-docs refuses quick runs (publish only full runs)." >&2
    exit 1
  fi
  python3 -I "$SCRIPT_DIR/gen_docs.py" "$OUT"
fi
echo "=== Benchmark run complete ==="
