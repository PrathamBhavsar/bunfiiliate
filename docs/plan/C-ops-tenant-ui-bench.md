# Plan C: ops, tenant audit, UI, bench

Status: design spec (no code). Baseline: `make ci` green on `prod`. Workers: gemini | kiro | opencode only.

## 0. Global rules (every task)

- Branch from `staging`, merge `--no-ff`. Never commit on `prod`/`staging`. Commit msgs short (`feat: x`), **no Co-Authored-By, no AI attribution**.
- Push only to remotes under `github.com/pratham-projects`, and only after C90 passes.
- Obey `brain/wisdom/anti-patterns.md` (esp. #4 no DB per tracker request, #5 shed with 503, #10 PG16 only, #9 no secrets).
- No paid services. Free/OSS only. No email dependency: in-app notifications + signed webhooks always; mail is an optional `Mailer` trait impl (SMTP / Resend free / Brevo free), **off by default** (`BF_MAIL=off`).
- Each task ends with: acceptance commands green, `make ci` green, brain updated (`brain/tasks/index.md`, top of `brain/log.md`, `brain/wisdom/` lessons), `python ~/.claude/skills/okf/scripts/validate.py brain --strict`.
- Env: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`. Tests needing PG use `DATABASE_URL` (PG16).
- Clean-room: the live reference app (`scratchpad/live`) is read for feature parity only. Copy nothing.

## 1. Baseline facts (verified)

- Crates: `api, core, db, rt, spool, tracker`. Migrations: `crates/db/migrations/{0001_init,0018_conversion_payout,0040_conversion_hold,0050_money_fixes}.sql`, run by `db::migrate` (`sqlx::migrate!`).
- Tenancy: `auth::Principal{user_id, role, workspace_id}`; `Principal::workspace_scope(requested)` is the single scoping gate. Extractor `Auth<R: Role>`.
- 64 route registrations across `crates/api/src/{affiliates,analytics,audit,auth,money,ops,payouts,programs,webhooks,workspaces}.rs`, assembled in `routes.rs`.
- Tables carrying `workspace_id` directly vs by parent: `clicks` has **no** `workspace_id` (scoped via `program_id -> programs.workspace_id`). `clicks.id` is the sole PK (blocks partitioning, see C20).
- Spool: `crates/spool` (`SpoolWriter`, segments, `write_checkpoint`, `gc`). Tracker sheds 503 via `try_reserve` (`tracker/src/lib.rs`). `BF_FSYNC` env.
- Web: `apps/web` Next static export (`output: "export"`, so no dynamic route params at runtime: use query-string routes). No per-code stats API/page (T34 note).
- Bench: `bench/run.sh` (oha, scratch DB `bf_bench`), results in `bench/results/*.json`, `docs/benchmarks.md`.
- E2E: `make e2e` -> `scripts/e2e.sh`, `e2e/tests/affiliate-flow.e2e.ts`.

## 2. Task DAG

```
C01 -> C02 -> C03 -> C04 -> C05
C10 -> C11 -> C12 ; C13 ; C14 -> C15 ; C16
C20 -> C21 ; C22 ; C23 -> C24
C30 -> C31 -> C32 ; C33 -> C34
C40 -> C41 -> C42
C90 depends on all
```
Tenant (C0x), Ops (C1x/C2x), UI (C3x), Bench (C4x) tracks are independent except noted.

---

## TRACK A: Tenant-leak audit

### C01 Route + query inventory (generated, checked in)
- Branch `feat/c01-route-inventory`; worker **gemini**; depends: none.
- Files: `crates/api/src/routes.rs` (expose `pub fn route_table() -> Vec<RouteInfo>`), new `crates/api/tests/tenant_inventory.rs`, new `docs/audit/tenant-inventory.md` (generated).
- Steps:
  1. Source of truth is the utoipa OpenAPI doc (`api openapi`): iterate `paths` -> `(method, path, security, path-param names)`.
  2. Test `inventory_matches_doc`: regenerates markdown table and compares to `docs/audit/tenant-inventory.md`; `UPDATE_INVENTORY=1` rewrites it.
  3. Doc table columns: method, path, auth role, id params, `workspace-scoped?` (manual annotation file `docs/audit/tenant-annotations.toml`: `[[route]] path, method, kind = "scoped"|"public"|"self"|"super"`, `reason`).
  4. Repo query inventory: script `scripts/audit-sql.sh` greps `sqlx::query*` literals in `crates/db/src` and `crates/api/src` into `docs/audit/queries.txt`.
- Acceptance: `cargo test -p api tenant_inventory`.
- Tests:
  - `every_openapi_route_is_annotated`: fails if a path/method lacks an annotation (new routes force a decision).
  - `annotations_have_no_orphans`: annotation with no route fails.
  - `public_routes_allowlist`: exact set == {`auth/login`, `auth/refresh`, `affiliates/register`, tracker routes not in api} (assert literal list).

### C02 Cross-tenant route matrix test
- Branch `feat/c02-tenant-matrix`; worker **opencode**; depends: C01.
- Files: new `crates/api/tests/tenant_matrix.rs`, helper `crates/api/tests/common/seed.rs`.
- Design: seed workspace A and B, each with admin user, affiliate user, program, site, plan, conversion type, affiliate, referral code, customer, conversion (pending), payout batch, webhook endpoint+delivery, email template, notification, contact request, audit row, ledger entries. Collect ids into `struct Ids { a: Ws, b: Ws }`.
- Runner: for each route annotated `scoped`: build request as **A admin** (and separately **A affiliate** for `/me/*`), substituting every id path param with the **B** id of the matching resource type (map `{id}`->resource by path prefix, table in test; `{code}`, `{domain}`, `{affiliate_id}`, `{plan_id}`, `{type_id}` too). Body for POST/PATCH/PUT from a `fixtures()` map keyed by `(method, path)` that references B ids.
- Assertions per request:
  - status in {404, 403}; never 200/2xx/409/422 that reveals existence (404 preferred; list of allowed 403 routes in annotations `expect = 403`).
  - response body contains no B uuid/strings (search for B marker strings like `B-SECRET-NAME`).
  - After all calls, **DB snapshot equality**: hash of all B rows (`SELECT ... ORDER BY id` per table) unchanged; A row counts unchanged for mutations that must not create (e.g. no new row under B).
- List endpoints (no id param): run as A, assert result set ids subset of A and contain zero B markers; also `?workspace_id=<B>` => 403.
- Super admin: `workspace_scope` test: super admin w/ `workspace_id=B` => 200 and only B rows.
- Acceptance: `cargo test -p api --test tenant_matrix -- --test-threads=1`.
- Tests:
  - `matrix_covers_every_scoped_route` (count executed == count annotated `scoped`, so the matrix cannot silently skip).
  - `matrix_cross_tenant_read_denied`, `matrix_cross_tenant_write_denied_and_no_side_effect`, `lists_never_contain_foreign_rows`, `workspace_id_query_param_forbidden_for_non_super`, `super_admin_can_scope_to_other_workspace`.
  - proptest `prop_random_id_param_never_200`: random uuid in each id slot => 404 (never 500).

### C03 SQL lint: missing workspace scope
- Branch `feat/c03-sql-lint`; worker **kiro**; depends: C01.
- Files: new crate-free script `scripts/sql-scope-lint.py` (stdlib only, run with `python -I`), `docs/audit/sql-lint-allowlist.toml`, `Makefile` target `sql-lint`, CI step in `.github/workflows/ci.yml` (job `rust`).
- Rules: scan Rust string literals passed to `sqlx::query*`/`query_as*` in `crates/db/src` + `crates/api/src`. For each statement touching a tenant table (list derived from migration: any table with `workspace_id`, plus child tables `programs-children`, `clicks`, `conversions`, `ledger_entries`, `payouts`, `payout_batches`, `affiliates`, `customers`, `referral_codes`, `webhook_*`, `notifications`, `audit_log`, `email_templates`, `contact_requests`, `daily_rollups`):
  - SELECT/UPDATE/DELETE must contain `workspace_id` OR a join/subquery to `programs`/`affiliates` filtered by workspace OR the statement's fn name is in the allowlist (`reason` required: worker/system loops, id-already-authorized lookups with `// scope:` comment).
  - Accepts inline `-- scope: <reason>` SQL comment as an escape hatch (counted, printed).
- Acceptance: `make sql-lint` exit 0; `python -I scripts/sql-scope-lint.py --self-test` exit 0.
- Tests (self-test fixtures in `scripts/sql-lint-fixtures/`):
  - `missing_scope_select_fails`, `scoped_select_passes`, `allowlisted_passes`, `allowlist_entry_without_reason_fails`, `stale_allowlist_entry_fails` (entry not matching any query).

### C04 Defense in depth: repo-layer scope guard
- Branch `feat/c04-scope-guard`; worker **opencode**; depends: C02, C03.
- Files: findings fixes in `crates/db/src/*.rs` and `crates/api/src/*.rs` (whatever C02/C03 uncover), `docs/audit/findings.md`.
- Steps: (1) run C02/C03, record every failure in `findings.md` (route, query, fix, test). (2) Fix each by adding `workspace_id` predicate in the db fn (signature takes `ws: Uuid`), never by handler-only check. (3) Each fix gets a regression test in the C02 matrix fixtures. (4) Add `anti-patterns.md` rule: "db fn that reads/writes tenant tables must take a workspace id".
- Acceptance: `cargo test -p api --test tenant_matrix`, `make sql-lint`, `make ci`.
- Tests: one named regression test per finding `leak_<route>_<n>`; `findings_md_has_no_open_items` (greps `status: open`).

### C05 Postgres RLS evaluation (optional, decision doc + flagged spike)
- Branch `feat/c05-rls-eval`; worker **gemini**; depends: C04.
- Files: `brain/decisions/0004-rls.md`, spike migration `crates/db/migrations/0060_rls_spike.sql.disabled` (NOT applied), `crates/db/tests/rls_spike.rs` (`#[ignore]`).
- Sketch: `ALTER TABLE programs ENABLE ROW LEVEL SECURITY; CREATE POLICY ws ON programs USING (workspace_id = current_setting('app.ws', true)::uuid);` with `SET LOCAL app.ws` per transaction; app connects as non-owner role `bf_app`.
- Evaluate: pooled `SET LOCAL` correctness (must be in tx), cost on `ledger_entries`, tracker/worker bypass role, super admin path, `clicks` needs `workspace_id` column. Output: ADR with decision (default recommendation: **do not enable by default; keep app-layer scoping + C02/C03; offer opt-in doc**).
- Acceptance: `cargo test -p db --test rls_spike -- --ignored` passes against PG16; ADR validated by okf.
- Tests: `rls_blocks_cross_ws_select_without_app_filter`, `rls_set_local_resets_after_commit`, `rls_owner_role_bypass_documented`.

---

## TRACK B: Ops

### C10 Backup + restore drill (pg_dump + WAL archive + spool)
- Branch `feat/c10-backup`; worker **kiro**; depends: none.
- Files: `deploy/backup/backup.sh`, `deploy/backup/restore.sh`, `deploy/backup/drill.sh`, `deploy/systemd/bf-backup.{service,timer}`, `docs/ops/backup-restore.md`, `Makefile` target `backup-drill`.
- Design:
  - Logical: `pg_dump -Fc` nightly to `$BF_BACKUP_DIR/pg/<ts>.dump` + `sha256`; keep N (default 14).
  - PITR (optional doc + config): `archive_mode=on`, `archive_command='test ! -f $BF_BACKUP_DIR/wal/%f && cp %p $BF_BACKUP_DIR/wal/%f'`; base backup via `pg_basebackup -Ft -z`.
  - Spool: `tar` of `$BF_SPOOL_DIR` taken with the tracker running: copy segments then `checkpoint` file **last-written first** is wrong; rule: copy checkpoint file BEFORE segments, so restored checkpoint <= any segment end (replay is idempotent via click id PK `ON CONFLICT DO NOTHING`; assert this).
  - Remote copy: optional `rclone`/`rsync` hook env `BF_BACKUP_REMOTE` (user-supplied, free).
  - `drill.sh`: creates scratch DB, restores latest dump + spool tar into temp dirs, starts tracker replay mode (or calls `spool::replay_from_checkpoint` via a test binary), compares row counts + ledger balance hash to source, prints PASS/FAIL, exits non-zero on mismatch.
- Acceptance: `make backup-drill` exit 0 on seeded local DB.
- Tests:
  - integration `scripts/tests/backup_drill.sh`: seed -> backup -> drop DB -> restore -> `ledger_entries` hash equal, `SUM(signed)=0` per entry group.
  - `restore_refuses_nonempty_target_without_force`.
  - `spool_restore_replay_is_idempotent` (Rust, `crates/spool/tests/restore_replay.rs`: replay twice, insert twice, row count unchanged).
  - `backup_checksum_mismatch_aborts_restore`.

### C11 Spool disk-full handling
- Branch `feat/c11-spool-disk`; worker **opencode**; depends: none.
- Files: `crates/spool/src/lib.rs` (+ new `crates/spool/src/space.rs`), `crates/tracker/src/lib.rs`, `crates/tracker/src/metrics.rs`, `crates/spool/tests/disk_full.rs`, `docs/ops/spool-disk.md`.
- Design:
  - Config: `BF_SPOOL_HIGH_WATER_PCT=85`, `BF_SPOOL_LOW_WATER_PCT=70`, `BF_SPOOL_MIN_FREE_BYTES=256MiB`, `BF_SPOOL_MAX_BYTES` (optional cap on total segment bytes).
  - `SpaceGuard` samples free bytes (statvfs via `rustix` or `fs4`, whichever is already in `Cargo.lock`/deny-ok; check `cargo deny`) every 1s in a background task + on ENOSPC; state machine `Ok -> Shedding` at high water, back to `Ok` below low water (hysteresis).
  - While `Shedding`: tracker `/c`, `/r/:code` keep redirect (302 still served, click not recorded counted as `dropped`), `/c` and `/v1/events` return **503 + `Retry-After: 5`**. Decision: events (money) 503, clicks 503 on `/c`; redirect `/r/` still redirects (user experience) and increments `bf_spool_shed_total{route}`.
  - ENOSPC from `write`: mark `Shedding`, return `SpoolError::Full` (no panic, no partial record: truncate to last good offset on next open, already handled by replay torn-tail rule; add assert).
  - Metrics: `bf_spool_free_bytes`, `bf_spool_bytes`, `bf_spool_shedding` (0/1), `bf_spool_shed_total{route}`.
  - Recovery: GC (`gc(checkpoint)`) runs on transition to Shedding to free segments below checkpoint.
- Acceptance: `cargo test -p spool disk_full -- --test-threads=1`; CI-safe: Linux uses tmpfs mount via `sudo`-less fallback below.
- Tests:
  - `space_guard_hysteresis` (pure unit, injected `FreeSpace` closure: 90% => Shedding, 80% stays, 69% => Ok).
  - `write_returns_full_on_enospc` using fault injection `Fault` (already in spool) simulating ENOSPC; assert no torn record after reopen.
  - `tiny_tmpfs_fills_and_recovers` (`#[cfg(target_os="linux")]`, `#[ignore]` unless `BF_TMPFS_TEST=1`; script `scripts/tmpfs-test.sh` does `mount -t tmpfs -o size=2m` in a `unshare -m` namespace/CI docker `--privileged`; macOS skipped with message). Assert: fill -> 503s -> checkpoint+gc frees -> accepts again, zero accepted-then-lost records (replay count == 2xx count).
  - tracker integration `shedding_returns_503_with_retry_after`, `redirect_still_302_while_shedding`, `metrics_report_shedding_gauge`.

### C12 Alerts: Prometheus rules + Alertmanager + webhook sink
- Branch `feat/c12-alerts`; worker **gemini**; depends: C11 (metric names).
- Files: `deploy/monitoring/prometheus.yml`, `deploy/monitoring/rules/bunfiiliate.rules.yml`, `deploy/monitoring/alertmanager.yml`, `deploy/monitoring/compose.monitoring.yml` (profile `monitoring`, images prom/prometheus, prom/alertmanager only), `crates/api/src/ops.rs` (+ route `POST /api/v1/alerts/webhook`, token-auth `BF_ALERT_TOKEN`, creates in-app notifications for workspace admins / super admins), `docs/ops/alerts.md`.
- Rules (each with `for:`, severity, runbook anchor): `TrackerDown` (up==0 5m), `SpoolShedding` (bf_spool_shedding==1 1m), `SpoolDiskLow` (free<10%), `SpoolLagHigh` (spool unflushed offset lag > N), `WriterErrors` (rate(bf_writer_errors_total)>0 5m), `DeadLetterGrowing`, `ApiErrorRate5xx>2%`, `WebhookDeliveryBacklog`, `PgDown`, `MigrationPending`. Use only metric names that exist in `crates/*/src/metrics.rs`; test enforces it.
- Alertmanager: single receiver `bf-webhook` -> `http://api:PORT/api/v1/alerts/webhook` (+ commented optional SMTP/Slack receivers); no email dependency.
- Acceptance: `scripts/check-alerts.sh` (uses `promtool check rules` / `amtool check-config` if installed, else docker; skip with explicit message otherwise), `cargo test -p api alerts`.
- Tests:
  - `rules_reference_only_exported_metrics` (Rust: parse rules yml with `serde_yaml`-free regex of `bf_[a-z_]+`, assert each appears in a `metrics.rs` registration; fails on typos).
  - `alert_webhook_requires_token` (401 without), `alert_webhook_creates_notification_for_each_admin`, `alert_webhook_resolved_status_marks_resolved`, `alert_webhook_rejects_oversized_body`.
  - `promtool_rules_unit.yml` rule unit tests (`promtool test rules`) for `SpoolShedding` and `TrackerDown`.

### C13 HA notes (docs only, free/self-host)
- Branch `feat/c13-ha-docs`; worker **opencode**; depends: C10.
- Files: `docs/ops/ha.md`, update `docs/self-hosting.md` link.
- Content: single-node baseline and RPO/RTO table; Postgres streaming replica (optional) with `primary_conninfo`, `pg_basebackup -R`, manual promote; reads (analytics) may point to replica via `BF_DATABASE_URL_RO` **(design note only; not implemented)**; tracker is stateless except spool: run 2 trackers behind Caddy with **separate spool dirs**, each with own worker (click id PK dedup makes it safe); api worker mode single-leader via advisory lock (`pg_try_advisory_lock`), note whether code already does it (verify in `worker.rs`, document truthfully); failure matrix; what is NOT supported (multi-region, auto-failover).
- Acceptance: `python ~/.claude/skills/okf/scripts/validate.py brain --strict`; link check `scripts/check-links.sh` (grep relative md links exist).
- Tests: `docs_links_resolve` (shell), review checklist in PR body.

### C14 Migration policy + upgrade-from-snapshot tests
- Branch `feat/c14-migration-policy`; worker **opencode**; depends: none.
- Files: `docs/ops/migrations.md`, `brain/decisions/0005-migrations.md`, `crates/db/tests/upgrade.rs`, `crates/db/tests/snapshots/{0001,0018,0040,0050}.sql` (schema+seed dumps via `pg_dump --schema-only` plus minimal seed data inserts, generated by `scripts/snapshot-schema.sh <N>`), `scripts/check-migrations.sh`.
- Policy: numbered `NNNN_name.sql`, strictly increasing, forward-only (no edits to applied files; no `down`), each new migration must be safe on a populated DB (no unguarded `NOT NULL` add, use `IF NOT EXISTS`, batched backfill), additive-first (expand/contract over two releases), PG16 only. Every release creates `snapshots/<last_migration>.sql`.
- `check-migrations.sh`: (1) filenames sorted, unique prefixes, no gaps rule relaxed (gaps allowed, documented); (2) `git diff origin/staging -- crates/db/migrations` shows no modification/deletion of existing files (only additions) in CI; (3) no `DROP TABLE|DROP COLUMN` without `-- allow-destructive: <reason>`.
- Acceptance: `cargo test -p db --test upgrade`, `scripts/check-migrations.sh`.
- Tests:
  - `upgrade_from_each_snapshot_reaches_head` (for each snapshot: create DB, load snapshot incl. `_sqlx_migrations` rows, run `db::migrate`, assert head version and `SELECT count(*)` of seed rows preserved).
  - `fresh_install_schema_equals_upgraded_schema` (pg_dump schema-only of fresh vs upgraded-from-0001 normalized diff empty).
  - `applied_migration_checksum_unchanged` (hash table in `crates/db/tests/checksums.txt`; editing an old file fails).
  - `check_migrations_rejects_edited_old_file` (script fixture).

### C15 CI job for migrations
- Branch `feat/c15-ci-migrations`; worker **kiro**; depends: C14.
- Files: `.github/workflows/ci.yml` (new job `migrations`, PG16 service), `Makefile` target `migrations-check`.
- Steps: job runs `scripts/check-migrations.sh` with `fetch-depth: 0`, then `cargo test -p db --test upgrade`.
- Acceptance: `make migrations-check` locally exit 0; `actionlint` if installed.
- Tests: `workflow_has_migrations_job` (grep test in `scripts/tests/ci_shape.sh`).

### C16 Retention + health endpoints (small ops hygiene)
- Branch `feat/c16-ops-hygiene`; worker **gemini**; depends: none.
- Files: `crates/api/src/ops.rs` (`GET /healthz`, `GET /readyz` checking PG ping + migration head; tracker `/healthz` exists? verify), `deploy/*` healthchecks.
- Tests: `healthz_200_no_auth`, `readyz_503_when_db_down` (closed pool), `readyz_reports_migration_head`. Add both to C01 annotation `kind="public"`.

### C20 Clicks partitioned by month (migration + retention)
- Branch `feat/c20-clicks-partition`; worker **opencode**; depends: C14.
- Files: `crates/db/migrations/0060_clicks_partition.sql`, `crates/db/src/tracker.rs` (insert unchanged: planner routes), `crates/db/src/ops.rs` or new `crates/db/src/partitions.rs`, `crates/api/src/worker.rs` (maintenance tick), `docs/ops/partitioning.md`.
- Constraints: PK must include partition key. New PK `(id, ts)`. Dedup now `ON CONFLICT (id, ts)`; click id derives from record so `ts` identical on replay (assert; ts comes from spooled record).
- SQL sketch:
  ```sql
  -- 0060: online-ish swap
  ALTER TABLE clicks RENAME TO clicks_old;
  CREATE TABLE clicks (LIKE clicks_old INCLUDING DEFAULTS INCLUDING CONSTRAINTS)
    PARTITION BY RANGE (ts);
  ALTER TABLE clicks ADD PRIMARY KEY (id, ts);
  CREATE TABLE clicks_default PARTITION OF clicks DEFAULT;
  CREATE INDEX clicks_ts_brin ON clicks USING brin (ts);
  CREATE INDEX clicks_affiliate_idx ON clicks (affiliate_id);
  CREATE INDEX clicks_visitor_idx ON clicks (program_id, visitor_id);
  -- FKs to programs/affiliates re-added (PG16 supports FKs from partitioned tables)
  -- create monthly partitions covering min(ts)..now()+2 months via DO block:
  --   CREATE TABLE clicks_y2026m11 PARTITION OF clicks FOR VALUES FROM ('2026-11-01') TO ('2026-12-01');
  INSERT INTO clicks SELECT * FROM clicks_old;  -- batched note: for large tables use docs/ops/partitioning.md procedure
  DROP TABLE clicks_old;
  ```
  Anything referencing `clicks(id)` as FK target must be checked (grep migrations; if any, re-point to `(id, ts)` or drop FK with ADR note).
- Partition manager (Rust fn `ensure_partitions(pool, months_ahead=3)`): idempotent `CREATE TABLE IF NOT EXISTS clicks_yYYYYmMM PARTITION OF ...`; called at api start and daily in worker tick. Rows landing in `clicks_default` raise metric `bf_clicks_default_rows` + alert (add to C12 rules).
- Retention job: `BF_CLICKS_RETENTION_MONTHS` (default `0` = keep forever). `drop_expired_partitions` detaches then drops partitions wholly older than cutoff; **never touches `daily_rollups`**; logs to `audit_log`.
- Acceptance: `cargo test -p db partitions`, `cargo test -p db --test upgrade` (snapshot 0050 -> head), `make ci`.
- Tests:
  - `ensure_partitions_idempotent`, `ensure_partitions_creates_next_n_months`.
  - `insert_routes_to_correct_partition` (`tableoid::regclass`), `out_of_range_goes_default_and_metric_increments`.
  - `replayed_click_dedups` (insert same id,ts twice -> 1 row).
  - `retention_drops_old_partition_keeps_rollups`, `retention_zero_keeps_everything`.
  - `migration_preserves_existing_clicks` (upgrade test from snapshot 0050 with 1000 clicks across 3 months: counts per month equal).
  - `analytics_queries_unchanged_results` (run existing analytics tests).

### C21 Partition-aware analytics sanity
- Branch `feat/c21-partition-queries`; worker **kiro**; depends: C20.
- Files: `crates/api/src/analytics.rs`, `crates/db/src/*` queries touching `clicks`.
- Steps: ensure every `clicks` query has a `ts` range predicate (enables pruning); add `EXPLAIN` test.
- Tests: `clicks_queries_prune_partitions` (EXPLAIN (FORMAT JSON) shows `Subplans Removed` or <=N scanned partitions for a 7-day window), `per_code_stats_query_has_ts_bound` (C30).

### C22 Rollup safety with retention
- Branch `feat/c22-rollup-retention`; worker **gemini**; depends: C20.
- Steps: confirm rollup worker finishes a day before partition drop: retention refuses to drop a partition if `rollup_state` (see 0050) watermark < partition upper bound. Doc it.
- Tests: `retention_skips_partition_not_yet_rolled_up`, `retention_logs_skipped_reason`.

### C23 (merged into C20; id reserved) / C24 Backup interplay
- C24: branch `feat/c24-backup-partitions`; worker **kiro**; depends: C10, C20. Update `backup.sh`/`drill.sh` so per-partition counts are verified; docs note that detached/dropped partitions are not in older dumps.
- Test: `drill_counts_per_partition`.

---

## TRACK C: UI

### C30 Per-code stats API
- Branch `feat/c30-code-stats-api`; worker **opencode**; depends: C04 (scope rules).
- Files: `crates/api/src/analytics.rs`, `crates/api/src/routes.rs` (utoipa registration, add response schemas in `components(schemas(...))`, anti-pattern #27), `crates/db/src/*` query, regenerate `sdks/ts/openapi.json` via `make openapi`.
- Routes:
  - `GET /api/v1/referral-codes/{code}/stats?from=YYYY-MM-DD&to=YYYY-MM-DD&granularity=day` (workspace admin; scope via code->program->workspace).
  - `GET /api/v1/me/referral-codes/{code}/stats?...` (affiliate; must own code, else 404).
  - `#[into_params(parameter_in = Query)]` on query struct (anti-pattern #24). Default range last 30d, max 366d (400 beyond).
- Response:
  ```json
  {"code":"abc","from":"2026-10-01","to":"2026-10-31","currency":"USD",
   "totals":{"clicks":120,"flagged_clicks":3,"conversions":9,"revenue_cents":90000,"commission_cents":9000},
   "series":[{"day":"2026-10-01","clicks":4,"conversions":0,"commission_cents":0}],
   "top_countries":[{"country":"US","clicks":70}],
   "top_referrers":[{"host":"news.example","clicks":20}]}
  ```
  Money integer cents, single program currency (anti-pattern #29). Counts clicks live from `clicks` with `ts` bounds (partition pruning) and conversions from `conversions`; no rollup dependency. Top lists limit 10.
- Acceptance: `cargo test -p api code_stats`, `make openapi-check`.
- Tests:
  - `code_stats_totals_match_seed`, `code_stats_excludes_other_codes`, `code_stats_zero_filled_series`, `code_stats_rejects_range_over_366d` (400), `code_stats_unknown_code_404`.
  - `me_code_stats_foreign_code_404` (other affiliate, other workspace), added to C02 matrix automatically via annotations.
  - `code_stats_openapi_query_params_are_query`.

### C31 Per-code stats pages
- Branch `feat/c31-code-stats-ui`; worker **gemini**; depends: C30, C32 (routing decision).
- Files: `apps/web/src/app/(app)/referral-codes/detail/page.tsx` (admin) and `apps/web/src/app/(app)/me/links/detail/page.tsx` (affiliate), `apps/web/src/components/code-stats.tsx`, `apps/web/src/lib/analytics.ts`, `apps/web/src/lib/nav.ts` (links from list rows).
- Behaviour: reads `?code=` via `useSearchParams()` inside `<Suspense>`; range picker (7/30/90d); KPI tiles; clicks/conversions chart (reuse existing analytics chart component); top countries/referrers tables; breadcrumbs; query key `["admin","code-stats",code,range]` and `["portal","code-stats",...]` (anti-pattern #30, never `"me"`).
- Acceptance: `pnpm -C apps/web typecheck lint build`, `make web`.
- Tests (Playwright, C34): `code_stats_page_renders_kpis`, `code_stats_page_unknown_code_shows_not_found_state`.

### C32 Static export routing review (decision)
- Branch `feat/c32-routing-adr`; worker **kiro**; depends: none.
- Files: `brain/decisions/0006-web-routing.md`, `apps/web/next.config.ts` (only if needed), list in `docs/ui/routes.md`.
- Decision to document: `output: "export"` cannot serve `/x/[id]` for runtime ids without `generateStaticParams`; options: (a) **query-string routes `?id=`/`?code=` (recommended; zero server, works with Caddy static + SPA fallback)**, (b) dynamic Next server (adds Node runtime; rejected), (c) hash routes. Table of every existing and planned detail page with chosen URL. Caddy: `try_files {path} {path}.html /index.html`; verify `deploy/Caddyfile`; ensure `trailingSlash` setting consistent with it.
- Acceptance: `make web`; manual `curl -I` against `caddy`/static server for `/referral-codes/detail?code=x` => 200.
- Tests: `static_export_contains_detail_pages` (script asserts `apps/web/out/referral-codes/detail.html` and `out/me/links/detail.html` exist after `make web`).

### C33 Empty/error/loading states + a11y smoke
- Branch `feat/c33-ui-states-a11y`; worker **opencode**; depends: none.
- Files: `apps/web/src/components/state.tsx` (`<EmptyState/>`, `<ErrorState onRetry/>`, `<LoadingRows/>`), update every `(app)/**/page.tsx` list (programs, affiliates, referral-codes, conversions, customers, ledger, payouts, webhooks, notifications, audit-log, contact-requests, email-templates, reports, analytics, me/*), `apps/web/src/lib/use-resource.ts` (expose `error`, `refetch`).
- Rules: every list has (1) skeleton while loading, (2) empty state with explanatory text and primary action when applicable, (3) error state showing server `message` (never raw JSON) + Retry, (4) 401 -> redirect login (existing), 403 -> "No access" state. Each page has `<h1 id="page-title">` (existing pattern), main landmark, labelled inputs, tables with `<caption>` or `aria-label`, buttons with accessible names, visible focus, colour contrast tokens from existing theme.
- a11y smoke: add `@axe-core/playwright` (MIT, free) in `e2e/`.
- Acceptance: `make web`, `make e2e`.
- Tests: `empty_state_shown_for_new_workspace` (fresh seed, each list route), `error_state_on_500` (route-mocked `page.route` returning 500, Retry refetches), `forbidden_state_on_403`, `axe_no_serious_violations_<page>` for login, dashboard, programs, affiliates, referral-codes, payouts, me, me/links (impact serious|critical => fail), `keyboard_tab_reaches_primary_nav`.

### C34 Playwright coverage plan + suite growth
- Branch `feat/c34-e2e-coverage`; worker **gemini**; depends: C31, C33.
- Files: `e2e/tests/*.e2e.ts` split (`admin-core`, `money`, `portal`, `code-stats`, `states-a11y`, `tenant-ui`), `e2e/fixtures/seed.ts`, `docs/ui/e2e-plan.md` (coverage table: page -> test), `scripts/e2e.sh` (shard-able, env `E2E_GREP`).
- Plan table must list each page in `apps/web/src/app` and the test id; test `coverage_table_lists_every_page` (node script compares table to `find ... -name page.tsx`).
- Tenant UI tests: `ws_a_admin_cannot_open_ws_b_code_page` (URL with B code => not-found state, no B data in DOM), `affiliate_cannot_view_other_affiliates_code_stats`.
- Constraints: deterministic, serial, no LLM, repeatable (existing T60a guarantees); each test creates own data with unique suffix.
- Acceptance: `make e2e` exit 0, run twice in a row.

---

## TRACK D: Benchmarks

### C40 Reproducible `bench/run.sh`
- Branch `feat/c40-bench-repro`; worker **kiro**; depends: none.
- Files: `bench/run.sh`, `bench/lib.sh`, `bench/hardware.json` (generated: cpu, cores, ram, os, pg version, rustc, oha version, git sha, fsync mode, spool fs type), `bench/results/*.json` schema doc `bench/README.md`.
- Changes: (1) pin `oha` version check (fail if different unless `BF_BENCH_ALLOW_OHA=1`); (2) do not `brew install` silently: print instruction and exit 2; (3) warm-up 3s discarded; (4) 5 repeats per endpoint x fsync mode, report median and min/max, 20 conc, 10s each (env-tunable `BENCH_DURATION`, `BENCH_REPEATS`, `BENCH_CONC`); (5) spool on a dedicated dir, record fs; (6) random-but-seeded code set (`BENCH_SEED`); (7) result JSON includes `schema_version`, `git_sha`, `hardware`, per-run raw samples; (8) fixed `DATABASE_URL` default replaced with required env or local default documented; remove hard-coded user `pratham` from the default (use `$USER`).
- Acceptance: `bench/run.sh --quick` exit 0 (BENCH_DURATION=2 repeats=1), `bench/run.sh` writes `bench/results/<date>-<sha>.json`.
- Tests: `bench/tests/test_run.sh`: `quick_run_writes_valid_json` (jq schema check), `missing_oha_exits_2`, `hardware_json_has_required_keys`, `error_rate_nonzero_fails_run`.

### C41 Regression thresholds + nightly CI
- Branch `feat/c41-bench-ci`; worker **opencode**; depends: C40.
- Files: `bench/thresholds.json`, `bench/compare.py` (stdlib; run with `python -I`), `.github/workflows/bench-nightly.yml` (`schedule: cron "17 3 * * *"` + `workflow_dispatch`; runner label `bench` self-hosted OR ubuntu-latest advisory-only), `bench/baseline/<runner-class>.json`.
- Rules: threshold = relative: fail if median rps < baseline x 0.80 or p99 > baseline x 1.5, **error count > 0 always fails**; baseline per hardware class key (`hardware.class`); on GitHub-hosted noisy runners the job is `continue-on-error: true` and posts only a summary (`$GITHUB_STEP_SUMMARY`) + in-app-free alert (workflow failure notification only). Baseline updates require a PR touching `bench/baseline/*` with reason (reviewed, CODEOWNERS-free rule: documented).
- Acceptance: `python -I bench/compare.py --self-test`; `python -I bench/compare.py bench/results/x.json bench/baseline/y.json` exit codes 0/1.
- Tests (`bench/tests/test_compare.py`, unittest): `passes_within_tolerance`, `fails_rps_drop_over_20pct`, `fails_p99_regression`, `fails_on_any_errors`, `missing_baseline_class_exits_3`, `noisy_runner_flag_downgrades_to_warning`.

### C42 benchmark docs rules
- Branch `feat/c42-bench-docs`; worker **gemini**; depends: C40, C41.
- Files: `docs/benchmarks.md` (generated section + rules section), `scripts/check-bench-numbers.sh` (existing, T51; extend to read new result schema), `site/index.html` numbers.
- Rules text: numbers published only from a `bench/run.sh` full run committed with `hardware.json` + git sha; always state hardware, `BF_FSYNC` mode, localhost single-machine caveat (keep NOTE); never mix fsync modes in one claim; "floor not capacity"; no comparison claims vs other products without same-hardware run; regeneration command `bench/run.sh --write-docs`.
- Acceptance: `scripts/check-bench-numbers.sh` exit 0 (site numbers == latest results), `make ci`.
- Tests: `check_bench_numbers_fails_on_drift` (edit a number in a temp copy), `docs_contains_hardware_block`.

---

## TRACK E: Final gate

### C90 Final gate (coordinator-run; worker: none, or opencode for scripting)
- Branch `feat/c90-final-gate` for `scripts/final-gate.sh` + `docs/ops/release-checklist.md` only; depends: all.
- `scripts/final-gate.sh` runs in order, stops at first failure, writes `.gate/report.md`:
  1. `make ci` (fmt, clippy `-D warnings`, test, deny, web, openapi-check)
  2. `make sql-lint` (C03) and `scripts/check-migrations.sh` + `cargo test -p db --test upgrade` (C14)
  3. `cargo test -p api --test tenant_matrix -- --test-threads=1` (C02) — **tenant matrix must pass, zero skips**
  4. `cargo test -p spool disk_full`, `scripts/check-alerts.sh`, `make backup-drill`
  5. `make e2e` run **twice** (repeatability)
  6. `bench/run.sh --quick` + `python -I bench/compare.py` (advisory locally; mandatory exit 0 for errors==0)
  7. `scripts/check-bench-numbers.sh`
  8. `ocr` review on the full diff `prod..HEAD` (the T60 gap: ocr was not re-run on full diff); every finding fixed or recorded in `docs/audit/findings.md` with reason
  9. `python ~/.claude/skills/okf/scripts/validate.py brain --strict`
  10. repo hygiene: `git log prod..HEAD --format=%B | grep -iE 'co-authored-by|claude|anthropic|codex'` must be empty; `git ls-files | grep -E '\.env$|\.mmdb$'` empty.
- Push policy (docs/ops/release-checklist.md):
  - No push until `final-gate.sh` exits 0 and `.gate/report.md` is committed to brain log.
  - Merge `feat/* -> staging --no-ff`, then `staging -> prod --no-ff`. Never commit on either.
  - Remote must match `github.com/pratham-projects/*`: `git remote get-url origin | grep -E 'github.com[:/]pratham-projects/'` else abort. Script `scripts/push-guard.sh` enforces; refuses force-push.
  - Push `prod` and `staging` only; tag `vX.Y.Z` after push.
- Tests: `final_gate_stops_on_first_failure` (stub steps), `push_guard_rejects_foreign_remote`, `push_guard_rejects_dirty_tree`, `hygiene_check_detects_attribution_line` (fixture commit message).

---

## 3. Suggested worker split (parallel waves)

| Wave | Tasks | Notes |
| --- | --- | --- |
| 1 | C01 gemini, C10 kiro, C11 opencode, C14 opencode, C32 kiro, C33 opencode, C40 kiro, C16 gemini | no inter-dependencies |
| 2 | C02 opencode, C03 kiro, C12 gemini, C15 kiro, C20 opencode, C41 opencode, C13 opencode | |
| 3 | C04 opencode, C05 gemini, C21 kiro, C22 gemini, C24 kiro, C30 opencode, C42 gemini | |
| 4 | C31 gemini, C34 gemini | |
| 5 | C90 | coordinator |

Migration numbering collision guard: only C20 adds a migration (`0060`); if another task needs one it takes the next free number and rebases. C05 spike is never applied.

## 4. Risks / open questions

- `clicks` PK change (C20) touches the writer's `ON CONFLICT`; verify `insert_clicks` conflict target in `crates/db/src/tracker.rs` first.
- tmpfs test needs Linux privileges; macOS runs unit + fault-injection variants only (documented, not silently skipped).
- Alert rule metric names depend on C11; keep C12 rules test as the guard.
- Bench numbers on shared GitHub runners are noisy: advisory only; real thresholds need a stable self-hosted runner (user-provided).
- Mail trait: if `crates/api/src/mail.rs` already exists, extend rather than create; keep default `off`.
