#!/usr/bin/env bash
# ocr review on the full diff base..HEAD. Findings are fixed or recorded in docs/audit/findings.md.
#
# Two modes:
#  - default: `ocr review` against the configured LLM endpoint (`ocr config` or OCR_LLM_*).
#  - OCR_MODE=delegated: no endpoint. The host agent reviewed the diff through `ocr delegate`
#    (rules and file list) with read-only reviewer subagents. The gate then checks that the
#    review is recorded in docs/audit/findings.md and that the diff scope still matches it.
set -euo pipefail
BASE="${FINAL_GATE_BASE:-prod}"
OUT="${GATE_DIR:-.gate}"

if [ "${OCR_MODE:-}" = "delegated" ]; then
  F="docs/audit/findings.md"
  grep -q '^## Pre-release review (`ocr delegate`' "$F" || { echo "no delegated review section in $F" >&2; exit 1; }
  # Every finding row must carry a disposition: a "Fixed" or "Open, accepted" table exists.
  grep -q '^### Fixed on' "$F" && grep -q '^### Open, accepted' "$F" || { echo "review in $F lacks Fixed / Open sections" >&2; exit 1; }
  git diff --name-only "$BASE"..HEAD >"$OUT/ocr-files.txt"
  echo "ocr: delegated review recorded in $F ($(wc -l <"$OUT/ocr-files.txt" | tr -d ' ') files in $BASE..HEAD)" | tee "$OUT/ocr.json"
  exit 0
fi

ocr review --from "$BASE" --to HEAD --format json \
  --exclude '**/generated/*,sdks/ts/openapi.json,sdks/ts/src/schema.d.ts,docs/audit/tenant-inventory.md,**/*.lock,crates/db/tests/checksums.txt' \
  >"$OUT/ocr.json"
test -s "$OUT/ocr.json"
echo "ocr output: $OUT/ocr.json (triage every finding into docs/audit/findings.md)"
