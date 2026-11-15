#!/usr/bin/env bash
# Tenant matrix must pass with zero skips (ignored tests count as skips).
set -euo pipefail
out="$(cargo test -p api --test tenant_matrix -- --test-threads=1 2>&1)" || { echo "$out"; exit 1; }
echo "$out"
echo "$out" | grep -E '^test result: ok\. [0-9]+ passed; 0 failed; 0 ignored' >/dev/null
