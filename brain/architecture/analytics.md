---
type: Architecture
title: Analytics Rollups and Reports
description: T23 worker rollup tick, watermark rule, analytics and CSV routes, scoping.
tags: [analytics, rollups, worker, api, csv]
timestamp: 2026-10-26
---

# Analytics Rollups and Reports

Code: `crates/api/src/analytics.rs` (tick + routes), tick wired into `worker::run` (60s interval). See [Worker & Ingest](/architecture/worker.md).

## Rollup tick
- `analytics::rollup(&pool)`: `pg_try_advisory_xact_lock` (busy = skip, returns 0), then per program one transaction-wide rebuild.
- Watermark = `max(day)` of the program's `daily_rollups`, minus 1 day (`LOOKBACK_DAYS`). Rows from that day on are deleted and re-inserted (`ON CONFLICT DO UPDATE` too), so reruns are idempotent and vanished buckets disappear. No rows yet = full history.
- Limits: a click/conversion backfilled behind the watermark, or a status change (reject/reverse) older than the window, is not seen until rollups for that program are cleared.
- Days are UTC. Clicks fold into `total|country|os|browser|device|referrer`. A conversion takes dims from `customers.first_click_id`'s click, `''` when none, so every dim sums to `total`. Only `pending|approved|paid` conversions count (rejected/reversed excluded). Integer cents only.

## Routes (`/api/v1`, any signed-in role)
- `GET /analytics/summary?from&to&program_id&affiliate_id&dim` -> totals + `conversion_rate_bps` (conversions*10000/clicks, truncated, 0 when no clicks; can exceed 10000 for recurring) + `breakdown` (only when `dim`; top 200 by clicks).
- `GET /analytics/timeseries` -> one point per day of the range, zero-filled.
- `GET /reports/analytics.csv?dim` (default `total`) -> `day,dim,dim_value,clicks,conversions,revenue_cents,commission_cents,conversion_rate_bps`; RFC 4180 quoting, leading `= + - @` prefixed with `'` (referrer is client-controlled).
- Defaults: `to` today, `from` to-29d; max 366 days; bad dates / from>to = 400.
- Scope: `programs::viewer` workspace; rows joined to `programs.workspace_id`. Foreign `program_id` = 404. Affiliates are forced to their own `affiliate_id`; naming another = 404. Admins may narrow with `affiliate_id`.
