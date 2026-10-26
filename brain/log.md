# Changelog

## [2026-10-26] T18 payouts + reversals

- **Creation**: `api::payouts::{create_batch, approve_batch, cancel_batch, mark_paid, batch_csv, export_csv}`, `bf_db::payouts::{owed, batch_rows}`, migration `0018_conversion_payout.sql` (`conversions.payout_id`). Worker ingest now handles refund/chargeback events (generic, Stripe, Paddle): pending is rejected, approved/paid is reversed with a balanced `AffPayable -> Expense` txn, never touching old ledger rows. Details: [Worker & Ingest](/architecture/worker.md).
- **Update**: `bf_core` `ConversionStatus::Paid` + `Reverse` is now valid (clawback after payout; table test and proptest invariant still hold). [Ledger](/architecture/ledger.md) and `docs/ledger.md` postings updated; `worker::post` is `pub(crate)`, takes `(conversion, payout)` ids and returns the txn id. [Tasks](/tasks/index.md) T18 done on branch. `cargo test -p api payouts`: threshold + no double-claim, paid flow balanced with `payable+pending == live - paid`, refund before approval, chargeback after payout nets next batch (append-only checked), reversal inside an open batch, provider refunds, cancel, double `mark_paid` rejected, exact CSV. Lessons: [Anti-Patterns](/wisdom/anti-patterns.md) 24, [Time-Traps](/wisdom/time-traps.md).

## [2026-10-26] T42 geoip

- **Creation**: `tracker::geo::Geo` (optional DB-IP lite country mmdb via `maxminddb` 0.32, `country.iso_code`). Click country = `CF-IPCountry` (valid) first, then mmdb lookup of the same client ip used for `ip_hash`, else null. `BF_GEOIP_DB` env path in `tracker` main: unset = disabled, set but unreadable = startup error. `tracker::start` takes a `Geo`; `AppState.geo: Arc<Geo>`. `scripts/fetch-dbip.sh [YYYY-MM]` downloads the month's `dbip-country-lite` mmdb.gz into gitignored `data/`. CC BY 4.0 attribution in README License section; `.env.example` documents the var.
- **Update**: [Tasks](/tasks/index.md) T42 done on branch. `cargo test -p tracker geo`: header-wins (db never asked), XX/T1/junk fall through, no ip = null, real mmdb lookup on a hand-built 1-node fixture, disabled, missing file. Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 22. Note: `crash_before_flush_replays_exactly_once` failed once in a full parallel run (passes alone and on rerun): timing-sensitive, not geo related.
## [2026-10-26] T17 review fix

- **Update**: [Worker & Ingest](/architecture/worker.md): attribution and the auto-approve tick now require an active affiliate enrolled in the program (security review: suspended/unenrolled affiliates could be credited or auto-approved). Test `suspended_or_unenrolled_affiliates_earn_nothing`.

## [2026-10-26] T17 worker ingest

- **Creation**: `api worker` and `api serve --with-worker` (`api::worker::{run, process_events, approve_due}`); `bf_core::{Attribution, FirstTouch, LastTouch, Touch}`; `bf_db::ledger::insert_entries` (txn-aware, `insert_txn` reuses it). Ingest claims events `FOR UPDATE SKIP LOCKED`, attributes by customer then touches, resolves plan (override, type, default) and duration window, writes conversion + pending ledger txn + outbox, marks the event; auto-approve tick moves pending to approved after `hold_days`. Details: [Worker & Ingest](/architecture/worker.md).
- **Update**: [Tasks](/tasks/index.md) T17 done on branch. `cargo test -p api flow_click_to_approved` plus `--test worker` (7: click to approved, hold/flag, duplicate, recurring windows, first vs last touch, code + override, bad events). Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 22.

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
