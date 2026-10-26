#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SITE_HTML="$ROOT_DIR/site/index.html"
RESULTS_DIR="$ROOT_DIR/bench/results"

if [ ! -f "$SITE_HTML" ]; then
  echo "Error: $SITE_HTML not found." >&2
  exit 1
fi

if [ ! -d "$RESULTS_DIR" ]; then
  echo "Error: $RESULTS_DIR not found." >&2
  exit 1
fi

echo "=== Verifying site/index.html benchmark numbers against bench/results/*.json ==="

python3 - "$ROOT_DIR" << 'EOF'
import json
import os
import re
import sys

root_dir = sys.argv[1] if len(sys.argv) > 1 else os.getcwd()
site_file = os.path.join(root_dir, "site/index.html")
results_dir = os.path.join(root_dir, "bench/results")

with open(site_file, "r", encoding="utf-8") as f:
    html = f.read()

benchmarks = [
    ("fsync_always_c", "fsync_always_c.json", "/c", "always"),
    ("fsync_always_r", "fsync_always_r.json", "/r/:code", "always"),
    ("fsync_always_webhook", "fsync_always_webhook.json", "/v1/events/:key", "always"),
    ("fsync_off_c", "fsync_off_c.json", "/c", "off"),
    ("fsync_off_r", "fsync_off_r.json", "/r/:code", "off"),
    ("fsync_off_webhook", "fsync_off_webhook.json", "/v1/events/:key", "off"),
]

errors = []

for bench_id, json_name, target, fsync in benchmarks:
    json_path = os.path.join(results_dir, json_name)
    if not os.path.exists(json_path):
        errors.append(f"Missing benchmark JSON: {json_path}")
        continue

    with open(json_path, "r", encoding="utf-8") as jf:
        data = json.load(jf)

    # Compute expected numbers matching bench/run.sh format_row
    rps_val = round(float(data["summary"]["requestsPerSec"]) * 100) / 100
    expected_rps = str(rps_val)
    if expected_rps.endswith(".0"):
        expected_rps = expected_rps[:-2]

    # Latencies
    if "metrics" in data and "latency_ms" in data["metrics"] and data["metrics"]["latency_ms"].get("p50") is not None:
        expected_p50 = f"{data['metrics']['latency_ms']['p50']} ms"
        expected_p99 = f"{data['metrics']['latency_ms']['p99']} ms"
    else:
        p50_val = round(data["latencyPercentiles"]["p50"] * 1000 * 100) / 100
        p99_val = round(data["latencyPercentiles"]["p99"] * 1000 * 100) / 100
        expected_p50 = f"{p50_val} ms"
        expected_p99 = f"{p99_val} ms"

    # Find the row in site/index.html
    row_match = re.search(r'<tr[^>]*data-benchmark=["\']' + re.escape(bench_id) + r'["\'][^>]*>(.*?)</tr>', html, re.DOTALL)
    if not row_match:
        errors.append(f"Row for benchmark '{bench_id}' not found in {site_file}")
        continue

    row_content = row_match.group(1)

    # Extract rps, p50, p99 from row
    rps_match = re.search(r'data-metric=["\']rps["\'][^>]*>([^<]+)<', row_content)
    p50_match = re.search(r'data-metric=["\']p50["\'][^>]*>([^<]+)<', row_content)
    p99_match = re.search(r'data-metric=["\']p99["\'][^>]*>([^<]+)<', row_content)

    if not rps_match:
        errors.append(f"[{bench_id}] rps metric not found in HTML row")
    else:
        actual_rps = rps_match.group(1).strip()
        if actual_rps != expected_rps:
            errors.append(f"[{bench_id}] RPS mismatch: HTML has '{actual_rps}', JSON expects '{expected_rps}'")

    if not p50_match:
        errors.append(f"[{bench_id}] p50 metric not found in HTML row")
    else:
        actual_p50 = p50_match.group(1).strip()
        if actual_p50 != expected_p50:
            errors.append(f"[{bench_id}] p50 mismatch: HTML has '{actual_p50}', JSON expects '{expected_p50}'")

    if not p99_match:
        errors.append(f"[{bench_id}] p99 metric not found in HTML row")
    else:
        actual_p99 = p99_match.group(1).strip()
        if actual_p99 != expected_p99:
            errors.append(f"[{bench_id}] p99 mismatch: HTML has '{actual_p99}', JSON expects '{expected_p99}'")

    print(f"✓ [{bench_id}] target={target} fsync={fsync}: rps={expected_rps}, p50={expected_p50}, p99={expected_p99}")

if errors:
    print("\nBenchmark verification failed with errors:", file=sys.stderr)
    for err in errors:
        print(f"  - {err}", file=sys.stderr)
    sys.exit(1)

print("\nAll benchmark numbers in site/index.html exactly match bench/results/*.json.")
EOF
