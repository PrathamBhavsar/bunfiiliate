# Architecture

- [System Layout](/architecture/system.md) — workspace tree, crate dependency direction, binaries
- [Tracker Hot Path](/architecture/tracker-hot-path.md) — snapshot, WAL spool, group commit, batch writer
- [Ledger & State Machines](/architecture/ledger.md) — postings, conversion/payout transitions
- [Auth & Tenancy](/architecture/auth.md) — cookies, refresh rotation, roles, workspace scope
- [Programs & Affiliates API](/architecture/api-routes.md) — T21 routes, scoping, tracker NOTIFY, referral-code rules
- [Web Shell & TS SDK](/architecture/web.md) — openapi pipeline, static-export app, dev proxy, ResourceView
- [Worker & Ingest](/architecture/worker.md) — T17 event processor, attribution, plan resolution, auto-approve
- [Analytics Rollups & Reports](/architecture/analytics.md) — T23 rollup tick, watermark, summary/timeseries/CSV routes
- [Ops Routes, Webhooks and Email](/architecture/ops.md) — T24 outbox dispatcher, signed webhooks + retry, lettre templates, notifications, contacts, settings, audit
- [Observability](/architecture/observability.md) — T41 Prometheus `/metrics`, series list, separate metrics port
