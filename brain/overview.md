---
type: Overview
title: bunfiiliate Overview
description: Product goals, scope, and non-goals for bunfiiliate.
tags: [bunfiiliate, overview, rust, affiliate]
timestamp: 2026-10-26
---

# bunfiiliate Overview

Open-source (AGPL-3.0), self-hostable affiliate tracking + commission engine. Clean-room: no code, schema text, or copy from any closed-source project.

## Goals
- Hot-path tracker: sub-ms click/conversion ingest, zero DB call per request, crash-safe via WAL spool.
- Money correctness: integer cents, append-only double-entry ledger, balances derived.
- Real product: multi-tenant (workspace > program > affiliate), JWT cookie auth, Stripe/Paddle/generic webhooks, payouts with approval + CSV.
- Showpiece: fraud rules, attribution strategies, Prometheus metrics, measured benchmarks, release page.
- Deploy: one VPS, docker compose or systemd + Caddy.

## Non-goals (v1)
- Kafka/Redis/NATS, GraphQL, ORM, BFF, FX, KMS, SSO/2FA/i18n, Kubernetes. See [ADR-0003](/decisions/0003-skip-list.md).

## Links
- [Architecture](/architecture/index.md) · [Task DAG](/tasks/index.md)
- Full plan: `docs/PLAN.md`
