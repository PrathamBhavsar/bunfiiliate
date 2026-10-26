---
type: Architecture
title: Tracker Hot Path
description: How the tracker ingests events durably with no DB call per request.
tags: [tracker, wal, ingest, idempotency]
timestamp: 2026-10-26
---

# Tracker Hot Path

1. Validate against in-memory `Snapshot` (`ArcSwap`): code -> program/affiliate/landing, program key -> secrets. Reload on Postgres `LISTEN bf_config`.
2. Mint UUIDv7 event id in Rust (not PG18 `uuidv7()`). Client IP is the TCP peer unless `BF_TRUSTED_PROXIES` names it (then the nearest untrusted `X-Forwarded-For` hop) or `BF_TRUST_CF=1` (`CF-Connecting-IP`): `tracker::proxy::Trust`. Normalize UA, referrer, country (`CF-IPCountry` only if `BF_TRUST_CF`, else optional DB-IP lite mmdb via `BF_GEOIP_DB`, else null: `tracker::geo`).
3. Send to spool writer, await oneshot ack. Writer drains queue, writes, fsyncs once per group, acks (group commit).
4. Acked batches -> bounded mpsc -> DB batch writer: `INSERT ... SELECT UNNEST(...) ON CONFLICT DO NOTHING`, then advance checkpoint. Delete segments below checkpoint.
5. Crash: replay from checkpoint. At-least-once + unique keys = effectively-once.
6. Channel full: 503 load shedding. Never unbounded memory. Clicks stop at 90% of the queue; the last tenth is for signed webhook events.
7. Spool write failures are all-or-nothing per group: every ack in the group gets an error, the file is truncated back to the group start. If the truncate (or a segment rotation) cannot be undone the writer is poisoned: all later writes fail until restart, so no acked record ever sits behind a hole and the checkpoint cannot skip over lost data.
8. Postgres rejecting a record for good (SQLSTATE 22/23) dead-letters it: the batch is retried row by row, bad rows are logged and counted (`bf_dead_lettered_total`), the checkpoint moves on. Transient errors are retried; replay at startup does the same, so one bad record cannot crash-loop the tracker.
9. Config reloads from `bf_config` NOTIFY are rate-limited, not delayed: the first change after a quiet spell reloads at once, later ones within 250 ms share one trailing reload.

Webhooks (generic/Stripe/Paddle): tracker only verifies signature (`tracker::webhook::Source::verify`, 5-minute tolerance for all three; generic signs `<X-Bf-Timestamp>.<Idempotency-Key>.<body>` so a captured request cannot be replayed under a new key) + spools to `inbound_events` through the same spool/queue/writer path as clicks (`Record::Event`). Dedup via `ON CONFLICT (program_id, source, external_id) DO NOTHING`, external_id = Idempotency-Key / Stripe `id` / Paddle `event_id`. Business logic lives in worker.

Fraud flags (T40): `AppState::click` runs `bf_core::fraud` rules (`IpVelocity`, `BotUa`, `Dedupe`) on the in-memory state, O(1), no DB. A flagged click is never dropped: it is spooled, stored and counted with `clicks.fraud_reason` (`ip_velocity|bot_ua|duplicate`, first match). All rules see every click so stateful windows stay accurate. Key maps rotate in two generations, so memory is bounded (about 2 x 50k keys per rule). Limits are `default_rules()` constants, not configurable yet. Known trade-off: a human double click inside the dedupe window flags the second click, and last-touch may attribute to it (the conversion is held until released).

fsync mode `always|off`, both benchmarked.

Related: [ADR-0001](/decisions/0001-wal-spool.md), [System Layout](/architecture/system.md).
