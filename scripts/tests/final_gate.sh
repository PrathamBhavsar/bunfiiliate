#!/usr/bin/env bash
# Tests for final-gate.sh, push-guard.sh, hygiene-check.sh. Run: bash scripts/tests/final_gate.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
ok() { echo "ok - $1"; }
bad() { echo "FAIL - $1"; fail=1; }

# final_gate_stops_on_first_failure
printf 'one|true\ntwo|exit 3\nthree|touch %s/ran-three\n' "$T" >"$T/steps"
GATE_DIR="$T/gate" FINAL_GATE_STEPS="$T/steps" "$ROOT/scripts/final-gate.sh" >/dev/null 2>&1; rc=$?
if [ $rc -ne 0 ] && [ ! -e "$T/ran-three" ] && grep -q 'FAIL (exit 3)' "$T/gate/report.md"; then ok final_gate_stops_on_first_failure; else bad final_gate_stops_on_first_failure; fi

# final_gate_passes_when_all_pass
printf 'one|true\ntwo|true\n' >"$T/steps"
if GATE_DIR="$T/gate2" FINAL_GATE_STEPS="$T/steps" "$ROOT/scripts/final-gate.sh" >/dev/null 2>&1 && grep -q 'ALL STEPS PASSED' "$T/gate2/report.md"; then ok final_gate_passes_when_all_pass; else bad final_gate_passes_when_all_pass; fi

# repo fixture for push guard + hygiene
R="$T/repo"; git init -q "$R"; cd "$R" || exit 1
git config user.email t@t; git config user.name t
echo a >a; git add a; git commit -qm "feat: base"; git branch -q prod
echo b >b; git add b; git commit -qm "feat: more"
mkdir -p "$T/g"; echo 'ALL STEPS PASSED' >"$T/g/report.md"

git remote add origin https://github.com/someone-else/x.git
if GATE_DIR="$T/g" "$ROOT/scripts/push-guard.sh" >/dev/null 2>&1; then bad push_guard_rejects_foreign_remote; else ok push_guard_rejects_foreign_remote; fi
git remote set-url origin https://github.com/pratham-projects/x.git
echo dirty >c
if GATE_DIR="$T/g" "$ROOT/scripts/push-guard.sh" >/dev/null 2>&1; then bad push_guard_rejects_dirty_tree; else ok push_guard_rejects_dirty_tree; fi
rm c
if GATE_DIR="$T/g" "$ROOT/scripts/push-guard.sh" --force >/dev/null 2>&1; then bad push_guard_rejects_force; else ok push_guard_rejects_force; fi
if GATE_DIR="$T/g" "$ROOT/scripts/push-guard.sh" >/dev/null 2>&1; then ok push_guard_accepts_clean_gated_tree; else bad push_guard_accepts_clean_gated_tree; fi
if GATE_DIR="$T/none" "$ROOT/scripts/push-guard.sh" >/dev/null 2>&1; then bad push_guard_requires_passing_report; else ok push_guard_requires_passing_report; fi

# hygiene
if "$ROOT/scripts/hygiene-check.sh" prod >/dev/null 2>&1; then ok hygiene_passes_clean_history; else bad hygiene_passes_clean_history; fi
echo c >c; git add c; git commit -qm "feat: x

Co-Authored-By: Someone <s@x.y>"
if "$ROOT/scripts/hygiene-check.sh" prod >/dev/null 2>&1; then bad hygiene_check_detects_attribution_line; else ok hygiene_check_detects_attribution_line; fi
exit $fail
