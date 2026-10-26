# Tasks

Source of truth for the DAG is `docs/PLAN.md` section 4. This page tracks status only.

| id | branch | status |
| :--- | :--- | :--- |
| T00 | feat/workspace-scaffold | done, merged to staging (Sonnet; cargo-deny ok) |
| T01 | feat/ci | done, merged (agy/antigravity) |
| T10 | feat/core-money-ledger | done, merged (Sonnet) |
| T11 | feat/core-state-machines | done, merged (Sonnet) |
| T12 | feat/db-schema | done, merged (Sonnet) |
| T13 | feat/spool | done, merged (agy/antigravity) |
| T14 | feat/tracker-hot-path | done, merged (Sonnet); tracker.js bf_click-as-code follow-up fixed in T16 |
| T15 | feat/tracker-js | done, merged (Sonnet) |
| T16 | feat/ingest-webhooks | done, merged (Sonnet) |
| T17 | feat/worker-ingest | todo |
| T18 | feat/payouts-reversals | todo |
| T20 | feat/api-auth | done, merged (Sonnet); JWT hand-rolled HS256 (rsa advisory), review in T60 |
| T21 | feat/api-programs-affiliates | done, merged (Sonnet); `cargo test -p api programs` 8 tests |
| T22-T25 | api routes, analytics, ops, SDK | todo (SDK pipeline already in T30) |
| T30 | feat/web-shell | done, merged (Sonnet); SDK pipeline + apps/web shell. CI uses pnpm 9: bump to 12 for the workspace lockfile |
| T31-T34 | web pages | todo |
| T40, T41, T43, T44 | fraud, observability, load test, deploy | todo |
| T42 | feat/geoip | done on branch (Sonnet); `cargo test -p tracker geo` 6 tests |
| T50 | feat/oss-docs | done, merged (agy/antigravity) |
| T51 | feat/release-page | todo |
| T60 | final gate (make ci, ocr, e2e) | todo |

Rules: [Pre-Merge Gate](/wisdom/pre-merge-gate.md). Decisions: [Decisions](/decisions/index.md).
