#!/usr/bin/env bash
# End-to-end suite: release api + tracker, web static export and Caddy on a scratch database, driven by `npx e2e`.
# Deterministic steps only (locators + expect): no LLM or API key.
set -euo pipefail

export PATH="/opt/homebrew/opt/postgresql@16/bin:/opt/homebrew/bin:/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
export E2E_TELEMETRY_DISABLED=1

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

for cmd in cargo psql pnpm npm caddy curl; do
  command -v "$cmd" >/dev/null 2>&1 || { echo "Error: required command '$cmd' is not on PATH." >&2; exit 1; }
done

BASE_URL="${DATABASE_URL:-postgres://pratham@localhost:5433/postgres}"
DB_NAME="bf_e2e"
SCRATCH_URL="${BASE_URL%/*}/$DB_NAME"

# Uncommon ports so a running dev stack does not collide.
WEB_PORT="${E2E_WEB_PORT:-18000}"
API_PORT=18080
TRACKER_PORT=18081

ADMIN_EMAIL="admin@e2e.test"
ADMIN_PASSWORD="e2e-admin-password"
WORKSPACE_NAME="E2E Workspace"
WORKSPACE_SLUG="e2e-workspace"
JWT_SECRET="e2e-secret-e2e-secret-e2e-secret-e2e"
BF_IP_SALT="e2e-ip-salt-e2e-ip-salt"

SPOOL_DIR="$(mktemp -d /tmp/bf_e2e_spool_XXXXXX)"
WORK_DIR="$(mktemp -d /tmp/bf_e2e_work_XXXXXX)"
PIDS=()

cleanup() {
  local exit_code=$?
  trap - EXIT INT TERM
  echo "=== Tearing down ==="
  for pid in "${PIDS[@]:-}"; do
    [ -n "$pid" ] && kill -INT "$pid" 2>/dev/null || true
  done
  sleep 1
  for pid in "${PIDS[@]:-}"; do
    [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null || true
    [ -n "$pid" ] && wait "$pid" 2>/dev/null || true
  done
  rm -rf "$SPOOL_DIR" "$WORK_DIR"
  psql "$BASE_URL" -c "DROP DATABASE IF EXISTS $DB_NAME WITH (FORCE);" >/dev/null 2>&1 || true
  exit "$exit_code"
}
trap cleanup EXIT INT TERM

wait_for() { # url
  for _ in $(seq 1 100); do
    curl -fs -o /dev/null "$1" 2>/dev/null && return 0
    sleep 0.2
  done
  echo "Error: $1 did not come up." >&2
  return 1
}

echo "=== Building release api + tracker ==="
cargo build --release -p api -p tracker --manifest-path "$ROOT_DIR/Cargo.toml"
API_BIN="$ROOT_DIR/target/release/api"
TRACKER_BIN="$ROOT_DIR/target/release/tracker"

echo "=== Building web static export ==="
(cd "$ROOT_DIR" && pnpm install --frozen-lockfile && pnpm -C apps/web build)

echo "=== Installing e2e + chromium ==="
(cd "$ROOT_DIR/e2e" && npm ci --no-audit --no-fund && npx e2e-web install chromium)

echo "=== Creating scratch database $DB_NAME ==="
psql "$BASE_URL" -c "DROP DATABASE IF EXISTS $DB_NAME WITH (FORCE);" >/dev/null
psql "$BASE_URL" -c "CREATE DATABASE $DB_NAME;" >/dev/null

export DATABASE_URL="$SCRATCH_URL"
export API_PORT TRACKER_PORT JWT_SECRET BF_IP_SALT SPOOL_DIR
export COOKIE_SECURE=0

echo "=== Bootstrapping admin ==="
"$API_BIN" bootstrap --email "$ADMIN_EMAIL" --password "$ADMIN_PASSWORD" --workspace "$WORKSPACE_NAME"

echo "=== Starting api (+worker), tracker, caddy ==="
BF_METRICS_ADDR=127.0.0.1:18101 "$API_BIN" serve --with-worker >"$WORK_DIR/api.log" 2>&1 &
PIDS+=($!)
BF_METRICS_ADDR=127.0.0.1:18100 "$TRACKER_BIN" >"$WORK_DIR/tracker.log" 2>&1 &
PIDS+=($!)

# Same-origin proxy: deploy/Caddyfile as shipped, plus a global block that turns the admin API off.
printf '{\n\tadmin off\n}\nimport %s/deploy/Caddyfile\n' "$ROOT_DIR" >"$WORK_DIR/Caddyfile"
SITE_ADDRESS=":$WEB_PORT" API_UPSTREAM="127.0.0.1:$API_PORT" TRACKER_UPSTREAM="127.0.0.1:$TRACKER_PORT" \
  WEB_ROOT="$ROOT_DIR/apps/web/out" \
  caddy run --config "$WORK_DIR/Caddyfile" --adapter caddyfile >"$WORK_DIR/caddy.log" 2>&1 &
PIDS+=($!)

wait_for "http://127.0.0.1:$TRACKER_PORT/healthz"
wait_for "http://127.0.0.1:$API_PORT/api/openapi.json"
wait_for "http://127.0.0.1:$WEB_PORT/login"

echo "=== Running e2e ==="
set +e
(
  cd "$ROOT_DIR/e2e"
  E2E_APP_URL="http://localhost:$WEB_PORT" \
  E2E_ADMIN_EMAIL="$ADMIN_EMAIL" \
  E2E_ADMIN_PASSWORD="$ADMIN_PASSWORD" \
  E2E_WORKSPACE="$WORKSPACE_SLUG" \
  npx e2e run
)
status=$?
set -e

if [ "$status" -ne 0 ]; then
  echo "=== e2e failed: service logs ===" >&2
  tail -n 30 "$WORK_DIR/api.log" "$WORK_DIR/tracker.log" "$WORK_DIR/caddy.log" >&2 || true
fi
exit "$status"
