#!/bin/sh
# Copy the shared kit from this site into another repo's site/ dir. Canonical copy lives in bunfiiliate.
# Usage: scripts/sync-kit.sh /path/to/other-repo/site
set -eu
src="$(cd "$(dirname "$0")/.." && pwd)"
dst="${1:?target site dir}"
mkdir -p "$dst/src/kit" "$dst/public/fonts" "$dst/scripts"
cp "$src"/src/kit/* "$dst/src/kit/"
cp "$src"/public/fonts/* "$dst/public/fonts/"
cp "$src"/scripts/gen-llms.js "$src"/scripts/sync-kit.sh "$dst/scripts/"
cp "$src"/vite.config.js "$dst/"
echo "kit synced to $dst"
