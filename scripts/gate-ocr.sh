#!/usr/bin/env bash
# ocr review on the full diff base..HEAD. Findings are fixed or recorded in docs/audit/findings.md.
set -euo pipefail
BASE="${FINAL_GATE_BASE:-prod}"
ocr review --from "$BASE" --to HEAD --format json \
  --exclude '**/generated/*,sdks/ts/openapi.json,sdks/ts/src/schema.d.ts,docs/audit/tenant-inventory.md,**/*.lock,crates/db/tests/checksums.txt' \
  >"${GATE_DIR:-.gate}/ocr.json"
test -s "${GATE_DIR:-.gate}/ocr.json"
echo "ocr output: ${GATE_DIR:-.gate}/ocr.json (triage every finding into docs/audit/findings.md)"
