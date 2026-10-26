---
type: Architecture
title: Tracker Hot Path
description: How the tracker ingests events durably with no DB call per request.
tags: [tracker, wal, ingest, idempotency]
timestamp: 2026-10-26
---

# Tracker Hot Path

1. Validate against in-memory `Snapshot` (`ArcSwap`): code -> program/affiliate/landing, program key -> secrets. Reload on Postgres `LISTEN bf_config`.
2. Mint UUIDv7 event id in Rust (not PG18 `uuidv7()`). Normalize UA, referrer, country (`CF-IPCountry`, else optional DB-IP lite mmdb via `BF_GEOIP_DB`, else null: `tracker::geo`).
3. Send to spool writer, await oneshot ack. Writer drains queue, writes, fsyncs once per group, acks (group commit).
4. Acked batches -> bounded mpsc -> DB batch writer: `INSERT ... SELECT UNNEST(...) ON CONFLICT DO NOTHING`, then advance checkpoint. Delete segments below checkpoint.
5. Crash: replay from checkpoint. At-least-once + unique keys = effectively-once.
6. Channel full: 503 load shedding. Never unbounded memory.

Webhooks (generic/Stripe/Paddle): tracker only verifies signature (`tracker::webhook::Source::verify`, 5-minute tolerance for Stripe/Paddle) + spools to `inbound_events` through the same spool/queue/writer path as clicks (`Record::Event`). Dedup via `ON CONFLICT (program_id, source, external_id) DO NOTHING`, external_id = Idempotency-Key / Stripe `id` / Paddle `event_id`. Business logic lives in worker.

fsync mode `always|off`, both benchmarked.

Related: [ADR-0001](/decisions/0001-wal-spool.md), [System Layout](/architecture/system.md).
