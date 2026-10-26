---
type: Decision
title: ADR-0003 Deliberate skip list
description: Features and infra intentionally left out of v1.
tags: [adr, scope, ponytail]
timestamp: 2026-10-26
---

# ADR-0003 Deliberate skip list

Skipped on purpose: Kafka/Redis/NATS, BFF, GraphQL, ORM, typestate generics on DB entities, FX (one currency per program), KMS, table partitioning (BRIN on `clicks.ts` for now), SSO/2FA/i18n, k6 (oha only), testcontainers (`#[sqlx::test]` on local Postgres), Kubernetes/Helm, separate worker binary, Code of Conduct and issue templates.

**Why**: smallest complete change. Revisit after v1 ships.

See [Overview](/overview.md).
