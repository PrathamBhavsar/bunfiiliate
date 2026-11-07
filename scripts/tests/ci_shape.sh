#!/usr/bin/env bash
# Shape checks on .github/workflows/ci.yml. Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/../.."
f=.github/workflows/ci.yml
grep -q '^  migrations:' "$f" || { echo "FAIL workflow_has_migrations_job"; exit 1; }
awk '/^  migrations:/{on=1;next} /^  [a-z]+:/{on=0} on' "$f" > /tmp/ci_mig.$$
for needle in 'postgres:16' 'fetch-depth: 0' 'scripts/check-migrations.sh' 'cargo test -p bf-db --test upgrade'; do
  grep -q -- "$needle" /tmp/ci_mig.$$ || { rm -f /tmp/ci_mig.$$; echo "FAIL migrations job lacks: $needle"; exit 1; }
done
rm -f /tmp/ci_mig.$$
echo "ci_shape: ok"
