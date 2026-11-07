#!/usr/bin/env bash
# Every relative markdown link in docs/ops/*.md and docs/self-hosting.md must resolve.
set -euo pipefail
cd "$(dirname "$0")/.."
bad=0
for f in docs/ops/*.md docs/self-hosting.md; do
  dir=$(dirname "$f")
  while IFS= read -r link; do
    target=${link%%#*}
    case $target in ''|http*|mailto:*) continue;; esac
    if [ ! -e "$dir/$target" ]; then echo "broken link in $f: $link"; bad=1; fi
  done < <(grep -oE '\]\([^)]+\)' "$f" | sed -E 's/^\]\((.*)\)$/\1/')
done
[ "$bad" = 0 ] && echo "check-links: ok"
exit "$bad"
