# Tasks

Source of truth for the DAG is `docs/PLAN.md` section 4. This page tracks status only.

| id | branch | status |
| :--- | :--- | :--- |
| T00 | feat/workspace-scaffold | done, merged to staging (Sonnet; cargo-deny ok) |
| T01 | feat/ci | done, merged (agy/antigravity) |
| T10 | feat/core-money-ledger | done, merged (Sonnet) |
| T11 | feat/core-state-machines | done on branch (Sonnet); `cargo test -p bf-core state` ok |
| T12 | feat/db-schema | done, merged (Sonnet) |
| T13 | feat/spool | done, merged (agy/antigravity) |
| T14 | feat/tracker-hot-path | todo |
| T15 | feat/tracker-js | done, merged (Sonnet) |
| T16 | feat/ingest-webhooks | todo |
| T17 | feat/worker-ingest | todo |
| T18 | feat/payouts-reversals | todo |
| T20-T25 | api routes, analytics, ops, SDK | todo |
| T30-T34 | web shell + pages | todo |
| T40-T44 | fraud, observability, geoip, load test, deploy | todo |
| T50-T51 | oss docs, release page | todo |
| T60 | final gate (make ci, ocr, e2e) | todo |

Rules: [Pre-Merge Gate](/wisdom/pre-merge-gate.md). Decisions: [Decisions](/decisions/index.md).
