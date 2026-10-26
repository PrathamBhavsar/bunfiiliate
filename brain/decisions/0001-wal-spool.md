---
type: Decision
title: ADR-0001 WAL spool over message broker
description: Durable ingest via local WAL + Postgres unique keys instead of Kafka/Redis.
tags: [adr, wal, ingest]
timestamp: 2026-10-26
---

# ADR-0001 WAL spool over message broker

**Decision**: tracker writes to a local segmented WAL (CRC framed, group-commit fsync), then a batch writer inserts into Postgres.
**Why**: single-VPS target, zero extra services, crash safety proven by kill-restart test, idempotency keys make replay safe.
**Trade-off**: tracker holds local disk state; horizontal scale needs per-node spool (fine, each replays its own).
**Cut if short**: segment rotation (single file). Never cut idempotency keys.

See [Tracker Hot Path](/architecture/tracker-hot-path.md).
