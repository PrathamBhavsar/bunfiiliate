# Tenant audit findings (C02 matrix + C03 lint)

C02 (tenant matrix) found no leaks; `crates/api/tests/known_leaks.txt` stays empty.
C03 flagged repo/API fns that touch tenant tables without a workspace predicate, relying on a
handler check alone. Fixed by taking `ws: Uuid` in the fn and putting it in the SQL (defense in depth).

| # | fn | query | fix | test | status |
|---|----|-------|-----|------|--------|
| 1 | `api::payouts::create_batch` + `db::payouts::owed` | program lock, owed sum, payout insert | `ws` param; `workspace_id` on program lock, owed, affiliate select | `leak_payout_create_batch_1` | status: closed |
| 2 | `api::payouts::{lock,approve_batch,cancel_batch,mark_paid}` | batch lock by id | `lock(c, ws, id)` joins `programs.workspace_id` | `leak_payout_batch_actions_2` | status: closed |
| 3 | `api::payouts::batch_csv` + `db::payouts::batch_rows` | batch existence, CSV rows | `ws` param, `pr.workspace_id = $2` | `leak_payout_batch_csv_3` | status: closed |
| 4 | `db::affiliates::count_codes` | codes per affiliate | `ws` param, join `affiliates.workspace_id` | `leak_db_code_and_delivery_fns_4` | status: closed |
| 5 | `db::programs::delete_offsite_codes` | delete codes of a program | `ws` param, `USING programs` workspace join | `leak_db_code_and_delivery_fns_4` | status: closed |
| 6 | `db::affiliates::delete_enrollment` (code delete) | delete codes | `USING programs` workspace join | existing enrollment tests | status: closed |
| 7 | `db::ops::requeue_delivery` | update delivery by id | `ws` param, join `webhook_endpoints.workspace_id` | `leak_db_code_and_delivery_fns_4` (compile guard) | status: closed |

No other flagged fn is reachable with a client-supplied id: the rest are system loops (worker,
dispatcher, rollup, tracker snapshot, secrets CLI) or self-scoped lookups, listed with reasons in
`docs/audit/sql-lint-allowlist.toml`.

## Pre-release review (`ocr delegate`, three read-only reviewers over `prod..HEAD`, 2026-11-16)

Scope: `crates/api`, `crates/db` and migrations; `crates/tracker`, `core`, `spool`, `rt`, `deploy/`; web app, site, CI/release workflows, docs. Every item below was traced to code by the reviewer; the first group was re-checked by hand before fixing.

### Fixed on `feat/release-gate-fix`

| # | where | defect | fix | test |
|---|-------|--------|-----|------|
| R1 | `deploy/Caddyfile` | `/v2/*` not routed to the tracker: in the shipped compose stack every v2 beacon, SDK event and v2 webhook hit the static file server | `handle /v2/*` | manual (compose), see release checklist |
| R2 | `crates/tracker/src/main.rs` + compose | compose passes unset `BF_GEOIP_DB` as `""`; tracker tried to open it and exited on first boot | empty env = unset (tracker and `Config::from_lookup`) | `empty_value_counts_as_unset` |
| R3 | `crates/api/src/worker.rs` | a refund shrank `payouts.amount_cents` after the line was `pending`/`submitted` at PayPal/Wise, so `settle_confirmed` booked less cash out than left the bank | no shrink once `rail_state` is `pending`/`submitted`; the clawback nets against the next batch | `payouts_reversal_after_the_rail_took_the_line_keeps_the_sent_amount` (fails without the fix) |
| R4 | `crates/db/src/billing.rs` | an event for any other subscription id naming a workspace (cancel, past_due or active) overwrote the stored subscription, so a foreign buyer in the same store could downgrade or hijack it | event applies only for the stored subscription (same provider and id), when none is stored, or as a new active purchase after the stored one is canceled | `billing_cancel_of_another_subscription_does_not_downgrade` |
| R5 | web security section, docs, site, SECURITY.md | UI and copy said "two-step sign-in" but login is password-only; TOTP only gates password reset | copy now says authenticator codes reset your own password; SECURITY.md lists 2FA at sign-in as missing | n/a |
| R6 | `apps/web/src/lib/revealed.ts` | one-time secrets (recovery codes, webhook secrets) survived logout in the same tab | `clearUserData` also clears the store | n/a |
| R7 | api and tracker | no SIGTERM handler: `docker stop` waited 10 s then SIGKILLed | `bf_rt::shutdown_signal` | n/a |
| R8 | root `Dockerfile` | stale copy without `curl`, so compose healthchecks failed | identical to `deploy/Dockerfile` | n/a |
| R9 | docs, site, README, compose | quickstart missing required env, `--password`, Rust 1.85, swapped metrics ports (tracker 9100, api 9101), `git pull staging`, bare-metal env missing `BF_MASTER_KEYS`/`BF_IP_SALT`, GeoIP in compose, optional vars not passed through | corrected; compose forwards `SIGNUP_ENABLED`, `BF_HSTS`, `BF_JWT_SECRETS`, `TAX_REPORT_THRESHOLD_CENTS`, `BF_V1_SUNSET` | n/a |
| R10 | `deploy/Caddyfile`, `ci.yml`, `site` | HSTS `includeSubDomains`; CI had no `permissions:`; site doc links pointed at `staging` | dropped; `contents: read`; `branch: "prod"` | n/a |
| R11 | `scripts/check-bench-numbers.sh`, `push-guard.sh` | gate step checked a deleted `site/index.html`; guard demanded a different org | now checks `site/src/brand.js` against `docs/benchmarks.md`; guard allows `PrathamBhavsar/*` | `bench/tests/test_docs.sh`, `scripts/tests/final_gate.sh` |

### Fixed on `fix-small-o9o11o12`

| # | where | defect | fix | test |
|---|-------|--------|-----|------|
| O9a | `crates/api/src/routes.rs` (`login`) | an address over 254 bytes made the `auth_throttle` key (`email|ip`) overflow that table's btree primary key, so login answered 500 | over-long addresses are refused with the same 401 as bad credentials before any DB work | `auth_long_email_is_refused_not_a_500` |
| O9b | `crates/api/src/worker.rs` (`partition_maintenance`) | `auth_throttle` rows were never purged, so the table grew without bound | the existing 6-hourly upkeep deletes rows idle past `RESET.cap` with no lock in force | `partition_upkeep_purges_stale_auth_throttle_rows` |
| O11a | webhook tester (`/webhooks/test`) | the tester signs the payload and posts a real sale to the live inbound endpoint with no warning | an explicit "Not a sandbox" note names the live endpoint, the conversion/commission it can create and suggests a test program | e2e `webhook tester sends a signed test event ...` |
| O11b | login page | it always linked "Create a workspace", though signup is off by default | new public `GET /api/v1/config` (`signup_enabled`); the link renders only when the API says signup is on | `config_reports_signup_enabled`, `config_reports_signup_disabled`, e2e `login hides Create a workspace when signup is disabled` |
| O12 | `bench/gen_docs.py` | `--write-docs` rendered the datasheet into the deleted `site/index.html` and crashed | writes `docs/benchmarks.md` and the `/r/:code` facts in `site/src/brand.js`; `--check` verifies both | `bench/tests/test_docs.sh` |

### Fixed after the first Docker run and a second review (2026-11-19/20)

| # | where | defect | fix | test |
|---|-------|--------|-----|------|
| D1 | `api/src/mail.rs`, `notify.rs`, `rt/src/metrics.rs` | compose passes unset `SMTP_URL` as `""`: the api crashed on every boot ("invalid SMTP_URL"), the same empty-env class as R2 | empty `SMTP_URL`, `SMTP_FROM`, `BF_METRICS_ADDR` count as unset | real `docker compose up`: api healthy, full click, sale, conversion flow |
| D2 | `deploy/docker-compose.yml` | tracker started before the api migrated and crash-looped twice on first boot | tracker `depends_on` api healthy | compose restarts=0 on a clean boot |
| D3 | `tracker/src/idem.rs`, `lib.rs` | a cancelled request left its key `Pending` forever; a guard that released on cancel could free a key whose detached spool write still landed | `Reservation` guard; commit or release is decided in the spawned ingest task | `dropped_reservation_frees_pending_key_but_not_committed`, v2 tests |
| D4 | `deploy/backup/backup.sh` | a refused second run removed the running backup's `gc.paused` through its EXIT trap | flag taken atomically (`noclobber`), trap installed only by the owner, removed only if it holds our pid | `backup_drill.sh` (live pid keeps its flag) |

Rows below marked FIXED were open items in the first review; the remaining ones are still open.

### Fixed (open items from the first review)

| # | where | issue | status |
|---|-------|-------|--------|
| O2 | `tracker/src/lib.rs`, `idem.rs` | in-flight idempotency key answers 200 duplicate; a 503 on the first attempt then loses the event | FIXED: pending/committed key state; tests: `in_flight_pending_state_and_failed_release`, `v2_in_flight_idempotency_retry_after_shedding` |
| O3 | `tracker/src/lib.rs` (`redirect` shed branch) | click shed under load returns `bf_click` for a click never spooled; affiliate loses credit | FIXED: `ref=<code>` in shed Location redirect; tests: `ref_appended`, `redirect_still_302_while_shedding_but_records_no_click` |
| O4 | `spool/src/lib.rs` | FIXED: a failed batch after a segment rotation goes back to the starting segment (truncate) and deletes the segments it created; the writer is not poisoned | `enospc_right_after_a_rotation_rolls_back_and_writer_survives` |
| O5 | `deploy/backup/*` | FIXED: stale `gc.paused` (pid in the flag, dead pid cleared by the next backup, INT/TERM trapped), `restore` checks `pg_restore --list` before `--force` drops the schema, dump written as `.partial` then renamed, password moved from argv to `PGPASSWORD` | `scripts/tests/backup_drill.sh` (junk dump, stale/live pid, pgx argv) |
| O6 | compose + `monitoring/prometheus.yml` | FIXED: compose sets `BF_METRICS_ADDR` per service (tracker 9100, api 9101, compose network only) and optional `BF_METRICS_TOKEN` | `docker compose config` with the monitoring profile |

### Open, accepted for v0.1.0 pre-release (each is a known limit, not a regression)

| # | where | issue | why deferred |
|---|-------|-------|--------------|
| O1 | login (`routes.rs`) | TOTP is not enforced at sign-in | new feature (API + UI + e2e); copy is now honest. Next minor. |
| O7 | migrations 0071, 0080 | non-`CONCURRENTLY` index builds; 0080 copies all of `clicks` in one transaction | fine on a fresh install; take a backup and a maintenance window before upgrading a large database. |
| O8 | `tax.rs`, `payout_rails.rs`, `payout_requests.rs` | 1099 report buckets by batch `paid_at`; failed rail lines retry with the same idempotency key; cancelling a batch leaves its payout request `approved` | rail payouts are experimental until live-tested (see release checklist). |
| O9 | `password_reset.rs`, affiliates/programs | reset lockout can be forced by anyone knowing the email; plan-limit race; public register can fill `max_affiliates` (the long-email login 500 and the unpurged `auth_throttle` are fixed above) | low impact, no data exposure. |
| O10 | `auth.rs` `csrf_exempt` | header presence skips CSRF; today blocked by the Origin check, no CORS | defense in depth only. |

Not reviewed: `crates/api/src/money.rs` reversal/approval paths and `mail.rs` (unchanged since the earlier audits), the e2e test bodies, remaining web components.
