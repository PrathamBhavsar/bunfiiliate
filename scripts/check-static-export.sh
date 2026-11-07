#!/usr/bin/env bash
# Run after `make web`: detail pages must exist in the static export (ADR-0006).
set -eu
out="${1:-apps/web/out}"
rc=0
for f in referral-codes/detail.html me/links/detail.html; do
  [ -f "$out/$f" ] || { echo "missing $out/$f" >&2; rc=1; }
done
exit $rc
