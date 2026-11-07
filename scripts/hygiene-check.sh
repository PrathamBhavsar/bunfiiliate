#!/usr/bin/env bash
# Repo hygiene: no attribution lines in commit messages since $1, no env files or mmdb committed.
set -euo pipefail
BASE="${1:-prod}"
if git log "$BASE..HEAD" --format=%B | grep -iE 'co-authored-by|claude|anthropic|codex'; then
  echo "attribution line found in commit messages since $BASE" >&2; exit 1
fi
if git ls-files | grep -E '(^|/)\.env$|\.mmdb$'; then
  echo "env or mmdb file is tracked" >&2; exit 1
fi
