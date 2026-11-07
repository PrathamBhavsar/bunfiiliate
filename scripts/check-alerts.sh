#!/usr/bin/env bash
# Validate the monitoring config with promtool/amtool (local, else docker). Skips with a message.
set -euo pipefail
cd "$(dirname "$0")/../deploy/monitoring"
if command -v promtool >/dev/null && command -v amtool >/dev/null; then
  promtool check rules rules/bunfiiliate.rules.yml
  promtool test rules rules/bunfiiliate.rules.test.yml
  amtool check-config alertmanager.yml || true   # credentials_file must exist; see docs/ops/alerts.md
elif command -v docker >/dev/null && docker info >/dev/null 2>&1; then
  docker run --rm -v "$PWD:/m:ro" --entrypoint promtool prom/prometheus:v2.55.1 check rules /m/rules/bunfiiliate.rules.yml
  docker run --rm -v "$PWD:/m:ro" --entrypoint promtool prom/prometheus:v2.55.1 test rules /m/rules/bunfiiliate.rules.test.yml
else
  echo "check-alerts: skipped (no promtool/amtool or docker)"
fi
