---
type: Decision
title: ADR-0002 Worker as an api mode
description: Background worker ships inside the api binary, not as a third binary.
tags: [adr, worker, outbox]
timestamp: 2026-10-26
---

# ADR-0002 Worker as an api mode

**Decision**: `api worker` or embedded `api serve --with-worker`. Jobs claimed with `FOR UPDATE SKIP LOCKED`; ticks guarded by `pg_try_advisory_lock`. Plain tokio intervals.
**Why**: one image, one config, embeds on a small VPS, splits later with no code change.
**Skipped**: job-queue crate, cron crate.

See [System Layout](/architecture/system.md).
