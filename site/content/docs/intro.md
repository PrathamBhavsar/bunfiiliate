---
title: Introduction
group: Start
order: 1
description: What bunfiiliate is, what runs where, and what it leaves out.
---

# bunfiiliate

Open-source affiliate tracking and commission engine in Rust. Self-host it on one VPS.

It has two jobs, and each has its own binary. The `tracker` answers clicks and conversion events without a database call, writing to a crash-safe spool first. The `api` binary, together with its worker, turns events into conversions and posts every movement of money to an append-only double-entry ledger.

> bunfiiliate is pre-release. There is no tagged release yet. Read the [FAQ](#/faq) before you rely on it.

## What runs

| Part | Port | Role |
| --- | --- | --- |
| `tracker` | 8081 | Clicks, beacons, conversion events, Stripe and Paddle webhooks. Metrics on 9100. |
| `api` | 8080 | Admin and affiliate APIs, OpenAPI at `/api/openapi.json`, background worker. Metrics on 9101. |
| PostgreSQL 16 | 5432 | Single source of truth, including the ledger. |
| Caddy | 80, 443 | TLS, static web UI, same-origin proxy for `/api` and tracker routes. |

## Model

Data is organized as workspace, program, affiliate and referral code. The roles are `super_admin`, `admin` and `affiliate`, and every query is scoped to a workspace.

## Non-goals

bunfiiliate does not use Kafka, Redis, NATS, GraphQL, an ORM or Kubernetes. It needs one Postgres and runs two binaries.
