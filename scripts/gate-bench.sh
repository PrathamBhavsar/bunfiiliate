#!/usr/bin/env bash
# Quick bench run. Request errors must be zero (hard fail). Throughput/latency regressions against
# a committed baseline of the same hardware class are advisory on a dev machine.
set -euo pipefail
bench/run.sh --quick
latest="$(ls -t bench/results/*.json | head -1)"
# Result against itself: only the errors-always-fatal rule can fail.
python3 -I bench/compare.py "$latest" "$latest"
for b in bench/baseline/*.json; do
  [ -e "$b" ] || continue
  python3 -I bench/compare.py "$latest" "$b" --noisy || echo "advisory: $b: exit $? (class mismatch or regression)"
done
