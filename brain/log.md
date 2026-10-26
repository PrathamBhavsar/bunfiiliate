# Changelog

## [2026-10-26] T34 web affiliate portal

- **Creation**: affiliate pages in `apps/web`: `/me` (30-day summary tiles from `analytics/summary` + clicks/commission bar charts from `analytics/timeseries`, no chart lib), `/me/links` (own codes: create, copy `origin/r/<code>`, deactivate), `/me/earnings` (status + program filters), `/me/payouts`, new `/me/profile` (payout email + minimum in integer cents via `me/affiliate`), public `/register` (`affiliates/register`, `?workspace=` prefill) linked from `/login`. Reuses `formatCents`/`formatBps` from `lib/admin.ts` (no new formatter) and `ResourceView`, `FormDialog`, `ConfirmDialog`, `StatCard`. No per-code stats: the API has none. Role guard already came from `NAV`; admins hitting `/me*` are sent to `/dashboard`. Details: [Web Shell](/architecture/web.md).
- **Update**: [Tasks](/tasks/index.md) T34 done on branch. Acceptance `pnpm -C apps/web build && typecheck && lint` passes. Browser-verified against `api serve` on a seeded PG16 db: register (and duplicate-email error), pending vs active affiliate, dashboard figures match seed, create/duplicate/copy/deactivate link, earnings pagination (36 rows), payouts, profile edit, admin redirect, API-down error state. Found and fixed one bug: see [Anti-Patterns](/wisdom/anti-patterns.md) 28.
## [2026-10-26] T32 web money pages

- **Creation**: admin `/conversions` (program, affiliate, status, from/to filters; detail sheet; approve/reject/reverse with confirm), `/customers` (program, affiliate filters; detail), `/ledger` (Entries with account + txn-id filters and detail, Balances; header toggle), `/payouts` (new batch dialog, list with program/status filters, batch sheet with payout lines, approve, mark paid, cancel, CSV download). `ResourceView` filters gain `type: "date"`; `lib/admin.ts` gains money types, `useLookups`, `formatTime`. No routes beyond the API's; money only through `formatCents`.
- **Update**: [Tasks](/tasks/index.md) T32 done on branch. `pnpm -C apps/web build && typecheck && lint` pass. Verified in Chrome against `api serve --with-worker` on a seeded DB (42 conversions, 2 programs, 3 affiliates): list, filters incl. date range and the `from > to` 400 shown as an error state, detail, approve, reverse, stale-row approve showing the API's 409 message, batch create, "nobody is owed" 409 in the dialog, approve, mark paid (ledger cash legs -210.00 / 210.00 aff payable, balances updated), cancel, CSV file saved with exact rows, entries by txn id, balances, customers. Lesson: [Time-Traps](/wisdom/time-traps.md) (cookies across localhost ports).

## [2026-10-26] T41 observability

- **Creation**: `bf_rt::metrics` (hand-rolled Prometheus text registry: static atomics `Counter`/`Gauge`/`Histogram`, `router`, `serve`; no new crates, only `axum` + `tokio/net` on `bf-rt`), `tracker::metrics`, `api::metrics`. Tracker: `bf_ingest_latency_seconds`, `bf_batch_size`, `bf_shed_total`, `bf_spool_bytes`, `bf_spool_lag`. API/worker: `bf_outbox_depth`, `bf_webhook_delivery_failures_total`, `bf_ingest_events_processed_total`, `bf_rollup_tick_seconds`, `bf_approve_tick_seconds`. Details: [Observability](/architecture/observability.md).
- **Update**: [Tasks](/tasks/index.md) T41 done on branch. `cargo test --workspace metrics` (4): histogram cumulative buckets, bearer-token route, api series present, tracker `/metrics` text has `bf_spool_lag` after a click. `crash_before_flush_replays_exactly_once` hit the known shared-PG `_sqlx_test_*` clash in a full run (passes alone): see [Time-Traps](/wisdom/time-traps.md).
## [2026-10-26] T44 deploy assets & self-hosting docs

- **Creation**: `deploy/` directory containing multi-stage `Dockerfile` (builds `api` and `tracker` release binaries plus web static export into a lean Debian Bookworm runtime with `libssl3` and `ca-certificates`), `deploy/docker-compose.yml` and symlink `deploy/compose.yml` (`postgres:16`, `tracker`, `api` with `--with-worker`, `caddy` serving web static export and reverse proxying `/api` to port 8080 and tracker routes `/r/*`, `/c`, `/t.js`, `/v1/events/*`, `/v1/webhooks/*` to port 8081), `deploy/Caddyfile`, systemd units (`deploy/systemd/bunfiiliate-api.service`, `bunfiiliate-tracker.service`, `bunfiiliate-worker.service`), comprehensive `.env.example` documenting all 14 environment variables read by the codebase, and `docs/self-hosting.md` covering VPS requirements, first-run `api bootstrap`, automatic TLS, same-origin reverse proxying, GeoIP/CF-IPCountry, backups, upgrades, and systemd installations.
- **Update**: [Tasks](/tasks/index.md) T44 done on branch. Acceptance passed: `caddy validate --config deploy/Caddyfile` (Valid configuration), `npx markdownlint-cli2 "**/*.md" "#node_modules" "#brain"` (0 issues). `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all pass.


## [2026-10-26] T22 money routes

- **Creation**: `api::money` (customers list/get, conversions list/get with program|affiliate|status|from|to filters, admin `approve|reject|reverse`, `ledger/entries`, `ledger/balances`, affiliate `me/earnings`) and routes in `api::payouts` (create/list/get batch, `approve`, `mark-paid`, `cancel`, `export.csv`, affiliate `me/payouts`). Details: [Money Routes](/architecture/api-routes.md).
- **Update**: `worker::settle` is now the single place a conversion changes status (status column, balanced ledger txn, open-payout shrink, outbox); the refund/chargeback handler, the auto-approve tick and the admin actions all call it. [Worker & Ingest](/architecture/worker.md). Fixed the OpenAPI bug: every `IntoParams` struct now has `#[into_params(parameter_in = Query)]`, so `page`, `limit`, `status`, filters are query params; `make openapi` regenerated `sdks/ts`, and the web `listParams` workaround was removed (call sites use `params.query`). [Tasks](/tasks/index.md) T22 done on branch. `cargo test -p api money_routes` (6, real PG16): role guards, workspace isolation, affiliate sees only own, approve/reject/reverse keep every txn balanced, create->approve->paid->CSV exact, OpenAPI declares no stray path params. Lessons: [Anti-Patterns](/wisdom/anti-patterns.md) 24, 27.
## [2026-10-26] T40 fraud rules

- **Creation**: `bf_core::fraud`: `FraudRule` trait (`check(&ClickFacts) -> Verdict`), `IpVelocity` (N clicks per ip hash per sliding window), `BotUa` (woothee category `crawler`), `Dedupe` (same ip+ua+code within a window), `first_flag`, `default_rules` (30 clicks/60s per ip, 5s dedupe). State is in-memory, bounded (two-generation key maps), one mutex, no DB. Tracker `AppState.fraud` runs the rules in `AppState::click`: a flagged click is stored and counted with `clicks.fraud_reason` (column already in 0001; `Click.fraud_reason` is `serde(default)` so old spool records replay). Migration `0040_conversion_hold.sql`: `conversions.hold_reason`. Worker ingest copies `click_flagged:<reason>` onto a new conversion whose customer's `first_click_id` is a flagged click (recurring sales of that customer too); `approve_due` skips held rows; `worker::release_hold(pool, conversion)` clears it (no route yet, not workspace scoped: the route must check). Details: [Tracker Hot Path](/architecture/tracker-hot-path.md), [Worker & Ingest](/architecture/worker.md).
- **Update**: [Tasks](/tasks/index.md) T40 done on branch. `cargo test fraud`: window expiry, sliding (not fixed) window, bounded keys/history, missing ip, all-rules-run, tracker flagged-but-stored (bot, duplicate), worker hold + auto-approve skip + release. `AppState` gained a field (tests that build it by hand updated). Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 27.
## [2026-10-26] T33 web analytics + ops

- **Creation**: `/analytics` (range/program/affiliate filters, stat cards, clicks/conversions/commission line charts as small multiples, dimension breakdown table with bars, daily data table), `/reports` (CSV download of `reports/analytics.csv`), `/webhooks` (endpoints CRUD, signing secret reveal, deliveries with redeliver), `/email-templates`, `/contact-requests` (status actions), `/audit-log` (entity filter, action search, diff view), `/settings` (JSON editor), `/notifications` (any role, mark read), `/dashboard` now live (30d). Hand-rolled SVG `LineChart` (`components/charts.tsx`, no new dependency; series colors `--series-1..3` blue/orange/aqua from the dataviz palette, one measure per chart, hover readout), `lib/analytics.ts`, `MetricCards`. Money is shown only for one currency: a program, or all programs when they share it; mixed shows an em dash and asks to pick a program. No routes beyond the API's.
- **Update**: [Tasks](/tasks/index.md) T33 done on branch. [Web Shell](/architecture/web.md) synced. Verified in Chrome against `api serve --with-worker` on a fresh DB (two programs USD/EUR, 960 clicks, 45 conversions, 2 webhook endpoints, contact requests): charts + hover, mixed-currency state, program filter, webhook detail + redeliver (dead -> pending), contact status change, audit rows, settings validation, affiliate guard (`/analytics` -> `/me`), affiliate notifications + mark read. The CSV endpoint was fetched from the page; the Download button was not clicked (no file saved). Lessons: [Anti-Patterns](/wisdom/anti-patterns.md) 27, [Time-Traps](/wisdom/time-traps.md).

## [2026-10-26] T23 analytics rollups

- **Creation**: `api::analytics`: `rollup` tick (advisory-locked, per-program watermark = last rolled-up day minus 1, delete + upsert, folds clicks and conversions into `daily_rollups` for dims total|country|os|browser|device|referrer) run every 60s by `worker::run`; `GET /analytics/summary`, `/analytics/timeseries`, `/reports/analytics.csv`. `programs::{viewer,require_program}` now `pub(crate)`. Details: [Analytics](/architecture/analytics.md).
- **Update**: [Tasks](/tasks/index.md) T23 done on branch. `cargo test -p api analytics` (5, real PG16): idempotent + incremental rollup, breakdown sums = totals for every dim, affiliate cannot read another affiliate, workspace isolation, exact CSV. `make openapi` regenerated `sdks/ts`. Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 24.
## [2026-10-26] T18 payouts + reversals

- **Creation**: `api::payouts::{create_batch, approve_batch, cancel_batch, mark_paid, batch_csv, export_csv}`, `bf_db::payouts::{owed, batch_rows}`, migration `0018_conversion_payout.sql` (`conversions.payout_id`). Worker ingest now handles refund/chargeback events (generic, Stripe, Paddle): pending is rejected, approved/paid is reversed with a balanced `AffPayable -> Expense` txn, never touching old ledger rows. Details: [Worker & Ingest](/architecture/worker.md).
- **Update**: `bf_core` `ConversionStatus::Paid` + `Reverse` is now valid (clawback after payout; table test and proptest invariant still hold). [Ledger](/architecture/ledger.md) and `docs/ledger.md` postings updated; `worker::post` is `pub(crate)`, takes `(conversion, payout)` ids and returns the txn id. [Tasks](/tasks/index.md) T18 done on branch. `cargo test -p api payouts`: threshold + no double-claim, paid flow balanced with `payable+pending == live - paid`, refund before approval, chargeback after payout nets next batch (append-only checked), reversal inside an open batch, provider refunds, cancel, double `mark_paid` rejected, exact CSV. Lessons: [Anti-Patterns](/wisdom/anti-patterns.md) 24, [Time-Traps](/wisdom/time-traps.md).
## [2026-10-26] T24 ops routes

- **Creation**: outbox dispatcher in the worker (`api::dispatch`): per-endpoint `webhook_deliveries`, HMAC-SHA256 `X-Bf-Signature`, exponential backoff, dead after 8 attempts; lettre `Mailer` (`SMTP_URL`/`SMTP_FROM`, unset = log-only) with `{{var}}` templates sent on `conversion.approved` / `payout.paid`; routes for webhook endpoints + deliveries + redeliver, email templates, notifications (own), contact requests (public create), settings, audit log. `bf_db::ops` repo. Details: [Ops Routes](/architecture/ops.md). `worker::run` now takes a `Mailer`.
- **Update**: [Tasks](/tasks/index.md) T24 done on branch. `cargo test -p api ops` (8 integration tests with a local axum receiver + 2 unit): signature verifies, 500 then success, dead + redeliver, inactive/other-topic/other-workspace endpoints get nothing, email via lettre stub transport, templates, contact flow, role guards, tenancy, audit without secrets. SDK regenerated (`make openapi`). `deny.toml` allows 0BSD (lettre `quoted_printable`). Lessons: [Time-Traps](/wisdom/time-traps.md).

## [2026-10-26] T42 geoip

- **Creation**: `tracker::geo::Geo` (optional DB-IP lite country mmdb via `maxminddb` 0.32, `country.iso_code`). Click country = `CF-IPCountry` (valid) first, then mmdb lookup of the same client ip used for `ip_hash`, else null. `BF_GEOIP_DB` env path in `tracker` main: unset = disabled, set but unreadable = startup error. `tracker::start` takes a `Geo`; `AppState.geo: Arc<Geo>`. `scripts/fetch-dbip.sh [YYYY-MM]` downloads the month's `dbip-country-lite` mmdb.gz into gitignored `data/`. CC BY 4.0 attribution in README License section; `.env.example` documents the var.
- **Update**: [Tasks](/tasks/index.md) T42 done on branch. `cargo test -p tracker geo`: header-wins (db never asked), XX/T1/junk fall through, no ip = null, real mmdb lookup on a hand-built 1-node fixture, disabled, missing file. Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 22. Note: `crash_before_flush_replays_exactly_once` failed once in a full parallel run (passes alone and on rerun): timing-sensitive, not geo related.
## [2026-10-26] T17 review fix

- **Update**: [Worker & Ingest](/architecture/worker.md): attribution and the auto-approve tick now require an active affiliate enrolled in the program (security review: suspended/unenrolled affiliates could be credited or auto-approved). Test `suspended_or_unenrolled_affiliates_earn_nothing`.

## [2026-10-26] T17 worker ingest

- **Creation**: `api worker` and `api serve --with-worker` (`api::worker::{run, process_events, approve_due}`); `bf_core::{Attribution, FirstTouch, LastTouch, Touch}`; `bf_db::ledger::insert_entries` (txn-aware, `insert_txn` reuses it). Ingest claims events `FOR UPDATE SKIP LOCKED`, attributes by customer then touches, resolves plan (override, type, default) and duration window, writes conversion + pending ledger txn + outbox, marks the event; auto-approve tick moves pending to approved after `hold_days`. Details: [Worker & Ingest](/architecture/worker.md).
- **Update**: [Tasks](/tasks/index.md) T17 done on branch. `cargo test -p api flow_click_to_approved` plus `--test worker` (7: click to approved, hold/flag, duplicate, recurring windows, first vs last touch, code + override, bad events). Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 22.
## [2026-10-26] T31 web admin core

- **Creation**: admin pages on the T30 blocks. `/programs` (list, create, detail sheet on `?id=`: edit settings, keys and secrets reveal + rotate, sites, commission plans, enrolled affiliates, delete), `/affiliates` (status filter, approve/suspend/reactivate, payout settings), new `/referral-codes` (program + affiliate filters, create, delete; added to `NAV`). Helpers in `src/lib/admin.ts` (`listParams`, `ok`, `refresh`, integer parsers, cents/bps formatters), `admin-ui.tsx`, `program-detail.tsx`. Cents and bps are parsed as whole numbers client-side and mirror the API ranges. No routes beyond the API's; conversion types and provider-secret edits have no UI.
- **Update**: [Tasks](/tasks/index.md) T31 done on branch. Verified in Chrome against `api serve` on a fresh DB (bootstrap admin, two registered affiliates): login, approve, create program (client validation shown), add site, add plan (range error then ok), enroll, rotate (secret changed), create referral code. Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 22.

## [2026-10-26] T50 feat/oss-docs

- **Creation**: `CONTRIBUTING.md` (clean-room rules, 3-tier git flow `prod` <- `staging` <- `feat/*`, short commit message style, `make ci`, local Postgres 16 on port 5433, `brain/` upkeep, doc linting); `SECURITY.md` (private vulnerability disclosure via GitHub Security Advisories, response SLA, version support table, threat model scope & exclusions); `docs/architecture.md` (system layout, crate dependency graph mermaid, tracker hot path sequence mermaid, double-entry ledger postings table & state transitions, background worker loops, auth & tenancy model derived strictly from `docs/PLAN.md` and `brain/architecture/*`); `.markdownlint.json` (MD013 disabled, MD024 siblings_only).
- **Update**: `README.md` (replaced stub with what/why, feature list, mermaid architecture diagram, quickstart with Makefile/CLAUDE.md commands, pre-release status, AGPL-3.0 license note, links to docs); `docs/PLAN.md` and `docs/ledger.md` table and heading lint fixes.
- **Validation**: `npx markdownlint-cli2 "**/*.md" "#node_modules" "#brain"` passes (0 issues across 7 files); `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` passes with 0 warnings/failures. [Tasks](/tasks/index.md) T50 marked done.
## [2026-10-26] T16 ingest webhooks

- **Creation**: `tracker::webhook::Source{Generic,Stripe,Paddle}::verify` (hmac, constant-time via `verify_slice`, no SDKs, no `subtle` crate needed): generic `X-Bf-Signature` + required `Idempotency-Key`; Stripe `t=,v1=` over `<ts>.<body>`; Paddle `ts=;h1=` over `<ts>:<body>`; 5-minute tolerance on both timestamped sources; several `v1`/`h1` accepted (secret rotation). `POST /v1/events/{key}` (202), `/v1/webhooks/{stripe,paddle}/{key}` (200 after spool fsync), 256KB body limit. Spool record is now untagged `Record{Click,Event}`; writer batches events via `bf_db::tracker::insert_events` (`ON CONFLICT (program_id, source, external_id) DO NOTHING`). external_id = Idempotency-Key / Stripe `id` / Paddle `event_id`. Secrets already in `Snapshot`.
- **Update**: T14 follow-up fixed: `tracker.js` posts `{code, click, vid, url, ref}` (`?ref=` -> code, `bf_click` -> click; cookies `bf_ref`, `bf_click`); `POST /c` takes optional `click`, a click-only beacon is acked 204 and not recorded (`/r` already did), no code and no click is 400. [Tasks](/tasks/index.md) T16 done on branch. `cargo test -p tracker webhook`: valid sig, bad sig 400, replay 400, duplicates and crash/replay = 1 row. Fixtures computed independently (Python hmac). Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 16.
## [2026-10-26] T21 api programs + affiliates

- **Creation**: `/api/v1` workspaces, members, programs (+sites, conversion-types, plans, secrets/rotate), affiliates (public register, approve/suspend/reactivate, payout), enrollments, referral codes (admin + `/me`). `bf_db::{workspaces,programs,affiliates}` repos, `bf_db::tracker::notify_config`, `api::{page,error::AppJson}`, `AppError::{NotFound,Conflict}` (unique/FK violations map to 409). Details: [Programs & Affiliates API](/architecture/api-routes.md).
- **Update**: [Tasks](/tasks/index.md) T21 done on branch. `cargo test -p api programs` (8 tests, real PG16): role guards, workspace isolation, plan validation, NOTIFY on code create/delete/enroll/rotate, pagination, audit without secrets. Lessons: [Anti-Patterns](/wisdom/anti-patterns.md) 16, [Time-Traps](/wisdom/time-traps.md).
## [2026-10-26] T30 web shell + TS SDK

- **Creation**: `sdks/ts` (`openapi.json`, generated `src/schema.d.ts`, `createApi()`), `make openapi` / `make openapi-check`, `make web` (wired into `make ci`). `apps/web`: static-export Next 16 shell, login, cookie session with refresh-once-on-401, role guard + role nav, `ResourceView`/`useResource`/`FormDialog`/`ConfirmDialog`/`StatCard`/`StatusBadge`, placeholder pages for every nav item, light/dark theme. pnpm workspace at repo root. Details in [Web Shell](/architecture/web.md).
- **Update**: [Tasks](/tasks/index.md) T30 done on branch. Lessons: [Anti-Patterns](/wisdom/anti-patterns.md) 16-18. Verified: typecheck, lint, build, openapi-check, and the auth API contract through the dev proxy; the UI was not clicked through in a browser (extension unavailable).

## [2026-10-26] Wave 2 merged to staging

- **Update**: T11 state machines, T14 tracker hot path, T20 api auth merged. [Tasks](/tasks/index.md) synced.

## [2026-10-26] T11 core state machines

- **Creation**: `bf-core::state`: `ConversionStatus`/`ConversionEvent`, `PayoutStatus`/`PayoutEvent`, `InvalidTransition`, `conversion_created`. `transition` returns `Result<(Status, Option<LedgerTxn>), InvalidTransition>`; events that post carry the `Money` amount. Conversion `MarkPaid` posts nothing (cash moves on payout paid). Negative amounts are `InvalidTransition`.
- **Update**: [Ledger](/architecture/ledger.md) signature synced. [Tasks](/tasks/index.md) T11 done. Proptest: random event sequences keep txns balanced and `payable + pending = live - paid`; exact transition tables tested. Lesson in [Anti-Patterns](/wisdom/anti-patterns.md).
## [2026-10-26] T14 tracker hot path

- **Creation**: `tracker` crate (lib + bin): `Snapshot` in `ArcSwap` (LISTEN `bf_config` reload), `GET /t.js` `/r/{code}` `/healthz` `/readyz`, `POST /c` (JSON any Content-Type, CORS + OPTIONS), click pipeline spool ack -> bounded mpsc -> `INSERT .. UNNEST .. ON CONFLICT DO NOTHING` -> checkpoint + GC, startup replay. `bf_db::tracker` (`Click`, `insert_clicks`, snapshot row loaders, `CONFIG_CHANNEL`). Env `BF_FSYNC=off|always` (default always).
- **Update**: [Tasks](/tasks/index.md) T14 done on branch. `cargo test -p tracker`: 1000 clicks = 1000 rows; crash before flush then replay = 1000; replay from checkpoint 0 again = still 1000; unknown code 404; full queue 503. Lessons: [Anti-Patterns](/wisdom/anti-patterns.md) 12-13.
- **Follow-up**: ip hash salt reuses `JWT_SECRET` (no new config). Snapshot ignores affiliate status (fraud, T40). `tracker.src.js` posts the `bf_click` query value as `code` to `/c`; after `/r` that is a click id, so it 404s harmlessly. Revisit in T15/T40.
## [2026-10-26] T20 api auth

- **Creation**: `crates/api` lib+bin: axum app, utoipa OpenAPI (`/api/openapi.json`, `api openapi`), `AppError` -> `{error:{code,message}}`, `/api/v1/auth/{login,refresh,logout,me}`, `Auth<R: Role>` (`Any|Admin|SuperAdmin|Affiliate`), `Principal::workspace_scope`, Origin middleware, audit helper, `api bootstrap` (flags or `BF_ADMIN_EMAIL`/`BF_ADMIN_PASSWORD`/`BF_WORKSPACE_NAME`). `bf_db::auth` repo (users, refresh rotation, audit). `cargo test -p api auth` passes (15 tests, real PG16; bootstrap serialized by advisory lock).
- **Update**: [Tasks](/tasks/index.md) T20 done on branch. Details in [Auth and Tenancy](/architecture/auth.md). Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 12.
- **Review fixes**: client strings now strip control chars (NUL in a beacon `vid`/`url`/`ref` failed the whole batch insert and the writer retried forever); codes with NULL `landing_url` are skipped in the snapshot query (one NULL row broke the load). Still open: writer retries forever on any permanent DB error (quarantine/row-by-row fallback), and `CF-Connecting-IP`/`X-Forwarded-For` are trusted unconditionally (needs a trusted-proxy list; matters for T40 IP velocity).

## [2026-10-26] Wave 1 merged to staging

- **Update**: T00, T01, T10, T12, T13, T15 merged. [Tasks](/tasks/index.md) synced.
- **Update**: [Environment](/environment.md): PG16 on port 5433, antigravity agent id. [Time-Traps](/wisdom/time-traps.md) extended.

## [2026-10-26] T10 core money + ledger

- **Creation**: bf-core `Money`, `Bps`, `Currency`, ID newtypes, `Account`, `Posting`, `LedgerTxn` (balanced by construction), `CommissionPlan`; `docs/ledger.md`; proptests + golden table.
- **Update**: [Tasks](/tasks/index.md) T10 done on branch. Lesson: sum postings in i128; sequential i64 checked_add rejects balanced txns whose partial sums overflow ([Anti-Patterns](/wisdom/anti-patterns.md)).

## [2026-10-26] T12 db schema

- **Creation**: `crates/db/migrations/0001_init.sql` (full PLAN section 2 schema, append-only + deferred balance triggers, `affiliate_balances` view), `bf_db::{connect, migrate, ledger}`, `#[sqlx::test]` suite. `cargo test -p bf-db` passes.
- **Update**: [Tasks](/tasks/index.md) T12 done. Ledger sign convention: debit > 0, credit < 0.
## [2026-10-26] T13 feat/spool

- **Creation**: `bf-spool` segmented write-ahead log. Record framing `[len u32 | crc32 | payload]`, `SpoolWriter` with group-commit async queue drain and fsync modes (Always|Off), segment rotation by size, atomic checkpoint write+rename, replay iterator stopping cleanly at torn/corrupt tails, and segment GC.
- **Update**: [Tasks](/tasks/index.md) T13 marked done. Tests pass: roundtrip, torn-tail replay, corrupt CRC, checkpoint+segment GC, concurrent group commit.
## [2026-10-26] T15 tracker.js

- **Creation**: `crates/tracker/js/tracker.src.js` + committed `tracker.min.js` (esbuild, `npm run build:js`), `scripts/size-check.mjs`, node tests in `scripts/index.js`.
- **Update**: [Tasks](/tasks/index.md) T15 done. Beacon is `application/json`; T14 `/c` needs CORS + JSON body. Learned: [Time-Traps](/wisdom/time-traps.md).
## [2026-10-26] T01 ci and dependabot

- **Creation**: `.github/workflows/ci.yml` (rust with postgres:16 and cargo-deny, web conditional, js size-check conditional), `.github/dependabot.yml` (cargo, npm, github-actions weekly).
- **Update**: `Makefile` ci target mirrors Rust CI steps (fmt, clippy, test, deny).
- **Validation**: actionlint passes with 0 errors, `make ci` passes.

## [2026-10-26] T00 workspace scaffold

- **Creation**: Cargo workspace (bf-core, bf-db, bf-spool, bf-rt, tracker, api), Makefile, deny.toml. fmt/clippy/test/deny pass.
- **Update**: [Tasks](/tasks/index.md) T00 done. Codex blocked by hooks prompt, so Sonnet built it ([Time-Traps](/wisdom/time-traps.md)).

## [2026-10-26] Initialization

- **Creation**: bundle bootstrapped: [Overview](/overview.md), [Architecture](/architecture/index.md), [Decisions](/decisions/index.md), [Wisdom](/wisdom/index.md), [Tasks](/tasks/index.md), [Environment](/environment.md).
- Source: `docs/PLAN.md` (Opus plan), session decisions (name, AGPL-3.0, own VPS, git flow).
