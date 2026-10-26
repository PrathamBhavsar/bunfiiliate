---
type: Architecture
title: Observability Metrics
description: T41 Prometheus text /metrics for tracker and api - series, hot-path cost, separate listener.
tags: [metrics, prometheus, tracker, worker]
timestamp: 2026-10-26
---

# Observability

`bf_rt::metrics` is a ~150-line hand-rolled registry (no `metrics` crates). Series are `static` atomics; recording is one `fetch_add`/`store`, no allocation and no lock on the hot path. Text is built only on scrape.

## Not on the public port

`/metrics` is never mounted on the tracker or API router. Each binary spawns a second listener on `BF_METRICS_ADDR`: tracker default `127.0.0.1:9100`, api (`serve` and `worker`) default `127.0.0.1:9101` (distinct so both run on one host). Set `BF_METRICS_TOKEN` to also require `Authorization: Bearer <token>` (use it when binding beyond loopback). A bind failure is logged, never fatal.

## Tracker (`tracker::metrics`)

- `bf_ingest_latency_seconds` histogram: spool-ack latency per accepted record (success only).
- `bf_batch_size` histogram: records per DB batch.
- `bf_shed_total` counter: 503s from a full writer queue.
- `bf_spool_bytes` gauge: spool end offset (bytes ever appended); `bf_spool_lag` = bytes - checkpoint, i.e. spooled but not yet in Postgres. Checkpoint gauge is set after each successful `checkpoint + gc`.

## API / worker (`api::metrics`)

- `bf_outbox_depth` gauge: `outbox` rows with `done_at IS NULL`, sampled each 1s dispatch tick (partial index).
- `bf_webhook_delivery_failures_total`, `bf_ingest_events_processed_total` counters.
- `bf_rollup_tick_seconds`, `bf_approve_tick_seconds` histograms (`worker::timed`).

Related: [Tracker Hot Path](/architecture/tracker-hot-path.md), [Worker & Ingest](/architecture/worker.md), [Ops](/architecture/ops.md).
