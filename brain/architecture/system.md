---
type: Architecture
title: System Layout
description: Cargo workspace, crate responsibilities, dependency direction, deployables.
tags: [architecture, cargo, crates]
timestamp: 2026-10-26
---

# System Layout

## Crates
- `bf-core` — pure domain (Money, Bps, ledger, state machines, commission math). No IO.
- `bf-db` — SQLx pool, migrations, repos, outbox, advisory locks.
- `bf-spool` — segmented WAL, CRC framing, group-commit fsync, replay, checkpoint.
- `bf-rt` — env config, tracing, Prometheus exporter, graceful shutdown.
- `tracker` (bin) — hot path: `/t.js`, `/r/:code`, `/c`, conversion + webhook ingest.
- `api` (bin) — `serve [--with-worker] | worker | openapi | bootstrap | seed-demo`.

## Dependency direction
`core <- db <- {tracker, api}`; `spool <- tracker`; `rt <- {tracker, api}`.

## Deployables
Two binaries, one image. Worker is a mode of `api`, not a third binary ([ADR-0002](/decisions/0002-worker-as-api-mode.md)).

## Frontend
`apps/web` Next.js static export, same-origin behind Caddy, talks to `/api/v1` directly. TS SDK generated from OpenAPI (`sdks/ts`).

See [Tracker Hot Path](/architecture/tracker-hot-path.md), [Ledger](/architecture/ledger.md).
