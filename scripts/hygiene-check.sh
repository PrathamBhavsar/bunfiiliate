#!/usr/bin/env bash
# Repo hygiene: no attribution lines in commit messages since $1, no env files or mmdb committed.
# Commits listed in docs/audit/hygiene-exceptions.txt (full hashes, accepted by the owner) are skipped.
set -euo pipefail
BASE="${1:-prod}"
EXC="$(dirname "$0")/../docs/audit/hygiene-exceptions.txt"
bad=0
for h in $(git rev-list "$BASE..HEAD"); do
  if [ -f "$EXC" ] && grep -qx "$h" "$EXC"; then continue; fi
  if git log -1 --format=%B "$h" | grep -iE 'co-authored-by|claude|anthropic|codex' >/dev/null; then
    echo "attribution line in $h" >&2; bad=1
  fi
done
[ $bad = 0 ] || exit 1
if git ls-files | grep -E '(^|/)\.env$|\.mmdb$'; then
  echo "env or mmdb file is tracked" >&2; exit 1
fi
