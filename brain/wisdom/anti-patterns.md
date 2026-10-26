---
type: AntiPattern
title: Banned Anti-Patterns
description: Rules that must hold in every change to bunfiiliate.
tags: [wisdom, rules]
timestamp: 2026-10-26
---

# Banned Anti-Patterns

1. Floats for money. Integer cents only.
2. Mutable balances. Derive from ledger entries.
3. UPDATE/DELETE on `ledger_entries`.
4. DB call per tracker request.
5. Unbounded channels or queues. Shed load with 503.
6. Copying code, schema text, or copy from any closed-source project (clean-room).
7. Committing directly to `prod` or `staging`. Merge `--no-ff` from `feat/*` only.
8. Pushing before the local final gate passes.
9. Committing `.env`, secrets, or the MaxMind DB. GeoIP = DB-IP lite or `CF-IPCountry`.
10. PG18-only SQL (`uuidv7()`). CI pins PG16.

See [Pre-Merge Gate](/wisdom/pre-merge-gate.md).
