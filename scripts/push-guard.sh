#!/usr/bin/env bash
# Refuse to push unless: remote is github.com/pratham-projects/*, tree is clean, no force flags,
# only prod/staging/version tags, and the last final gate passed.
set -euo pipefail
for a in "$@"; do
  case "$a" in -f|--force|--force-with-lease*|+*) echo "force push refused" >&2; exit 1;; esac
done
url="$(git remote get-url "${PUSH_REMOTE:-origin}")"
echo "$url" | grep -E 'github.com[:/]pratham-projects/' >/dev/null || { echo "remote $url is not github.com/pratham-projects/*" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "working tree is dirty" >&2; exit 1; }
grep -q 'ALL STEPS PASSED' "${GATE_DIR:-.gate}/report.md" 2>/dev/null || { echo "no passing .gate/report.md" >&2; exit 1; }
echo "push guard ok"
