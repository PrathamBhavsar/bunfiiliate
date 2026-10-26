# Changelog

## [2026-10-26] T20 api auth

- **Creation**: `crates/api` lib+bin: axum app, utoipa OpenAPI (`/api/openapi.json`, `api openapi`), `AppError` -> `{error:{code,message}}`, `/api/v1/auth/{login,refresh,logout,me}`, `Auth<R: Role>` (`Any|Admin|SuperAdmin|Affiliate`), `Principal::workspace_scope`, Origin middleware, audit helper, `api bootstrap` (flags or `BF_ADMIN_EMAIL`/`BF_ADMIN_PASSWORD`/`BF_WORKSPACE_NAME`). `bf_db::auth` repo (users, refresh rotation, audit). `cargo test -p api auth` passes (14 tests, real PG16).
- **Update**: [Tasks](/tasks/index.md) T20 done on branch. Details in [Auth and Tenancy](/architecture/auth.md). Lesson: [Anti-Patterns](/wisdom/anti-patterns.md) 12.

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
