#!/usr/bin/env bash
# Final gate (C90). Runs every step in order, stops at the first failure, writes .gate/report.md.
# Override the step list with FINAL_GATE_STEPS=<file of "name|command" lines> (used by tests).
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
BASE="${FINAL_GATE_BASE:-prod}"
GATE_DIR="${GATE_DIR:-$ROOT/.gate}"
mkdir -p "$GATE_DIR"
REPORT="$GATE_DIR/report.md"

default_steps() {
  cat <<EOF
ci|make ci
sql-lint|make sql-lint
migrations|scripts/check-migrations.sh && cargo test -p bf-db --test upgrade
tenant-matrix|scripts/gate-tenant-matrix.sh
spool-disk-full|cargo test -p bf-spool disk_full
alerts|scripts/check-alerts.sh
backup-drill|make backup-drill
e2e-1|make e2e
e2e-2|make e2e
bench-quick|scripts/gate-bench.sh
bench-numbers|scripts/check-bench-numbers.sh
ocr|scripts/gate-ocr.sh
brain|python3 \$HOME/.claude/skills/okf/scripts/validate.py brain --strict
hygiene|scripts/hygiene-check.sh $BASE
EOF
}

if [ -n "${FINAL_GATE_STEPS:-}" ]; then STEPS="$(cat "$FINAL_GATE_STEPS")"; else STEPS="$(default_steps)"; fi

{
  echo "# Final gate report"
  echo
  echo "- date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "- head: $(git rev-parse --short HEAD) ($(git rev-parse --abbrev-ref HEAD))"
  echo "- base: $BASE"
  echo
  echo "| step | result | seconds |"
  echo "| --- | --- | --- |"
} >"$REPORT"

while IFS='|' read -r name cmd; do
  [ -z "$name" ] && continue
  echo "=== gate: $name ==="
  start=$(date +%s)
  bash -c "$cmd" >"$GATE_DIR/$name.log" 2>&1
  rc=$?
  secs=$(( $(date +%s) - start ))
  if [ $rc -ne 0 ]; then
    echo "| $name | FAIL (exit $rc) | $secs |" >>"$REPORT"
    tail -n 30 "$GATE_DIR/$name.log" >&2
    echo "gate FAILED at step: $name (log: $GATE_DIR/$name.log)" >&2
    echo >>"$REPORT"; echo "FAILED at $name" >>"$REPORT"
    exit 1
  fi
  echo "| $name | ok | $secs |" >>"$REPORT"
done <<<"$STEPS"

echo >>"$REPORT"; echo "ALL STEPS PASSED" >>"$REPORT"
echo "gate passed; report: $REPORT"
