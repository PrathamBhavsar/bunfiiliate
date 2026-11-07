#!/usr/bin/env bash
# Fixture tests for scripts/check-migrations.sh. Run: scripts/tests/check_migrations_test.sh
set -euo pipefail
script=$(cd "$(dirname "$0")/.." && pwd)/check-migrations.sh
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
cd "$t"
git init -q -b staging . && git config user.email t@t && git config user.name t
mkdir m && echo 'CREATE TABLE a (id int);' > m/0001_a.sql
git add . && git commit -qm base
export MIGRATIONS_DIR=m BASE_REF=staging
git checkout -qb feat

ok()   { "$script" >/dev/null 2>&1 || { echo "FAIL (expected ok): $1"; exit 1; }; }
bad()  { ! "$script" >/dev/null 2>&1 || { echo "FAIL (expected reject): $1"; exit 1; }; }

ok "clean tree"
echo 'CREATE TABLE b (id int);' > m/0002_b.sql; git add .; git commit -qm add
ok "added file"
echo '-- edit' >> m/0001_a.sql; git commit -qam edit
bad "check_migrations_rejects_edited_old_file"
git reset -q --hard HEAD~1
git rm -q m/0001_a.sql; git commit -qm del
bad "deleted old file"
git reset -q --hard HEAD~1
echo 'DROP TABLE a;' > m/0003_c.sql; git add .; git commit -qm drop
bad "unannotated drop"
printf -- '-- allow-destructive: unused table\nDROP TABLE a;\n' > m/0003_c.sql; git commit -qam ann
ok "annotated drop"
echo 'SELECT 1;' > m/0003_dup.sql; git add .; git commit -qm dup
bad "duplicate prefix"
echo "check_migrations_test: ok"
