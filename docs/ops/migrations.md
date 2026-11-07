# Migration policy

Files live in `crates/db/migrations/NNNN_name.sql` and are embedded in the binary (`bf_db::migrate`).

- Numbered, strictly increasing, unique prefixes. Gaps are allowed (numbers are reserved per task in `docs/plan/REVISIONS.md`).
- Forward-only: no `down`, and an applied file is never edited or deleted. sqlx checks checksums, and `crates/db/tests/checksums.txt` pins them. After adding a file run `UPDATE_CHECKSUMS=1 cargo test -p bf-db --test upgrade` and commit the new line.
- Safe on a populated DB: no unguarded `NOT NULL` add (add nullable or with a default, backfill in batches, then constrain), `IF NOT EXISTS` where re-runs are plausible.
- Additive first (expand/contract over two releases). `DROP TABLE` / `DROP COLUMN` needs a line `-- allow-destructive: <reason>` in the file.
- PostgreSQL 16 only. No PG18-only SQL.
- Every release adds a snapshot: `scripts/snapshot-schema.sh <last_migration>` writes `crates/db/tests/snapshots/<N>.sql` (schema + seed rows + `_sqlx_migrations`). Add it to `SNAPSHOTS` in `crates/db/tests/upgrade.rs`.

## Checks

- `scripts/check-migrations.sh`: names, ordering, no change to existing files vs `origin/staging` (`BASE_REF` overrides), destructive-SQL annotation.
- `cargo test -p bf-db --test upgrade`: every snapshot upgrades to head with seed rows kept; fresh schema equals upgraded schema; checksums unchanged.
- `make migrations-check` runs all of it; CI job `migrations` does the same with PG16.

## Health

`GET /healthz` is liveness. `GET /readyz` is 200 only when Postgres answers and the applied migration head is at least the head embedded in the binary; otherwise 503. Neither needs auth. Compose healthchecks hit `/readyz` on api and tracker.
