---
title: FAQ
group: Reference
order: 9
description: Status, performance claims, licensing and payouts.
---

# FAQ

## Is it production ready?

Not yet. It is pre-release and under active development. The ledger and tracker have property tests and benchmarks, but there is no tagged release.

## What does sub-millisecond mean?

With `BF_FSYNC=off` the tracker measured about 0.4 ms p50 on one machine (27.6k requests per second on `/r/:code`). With `BF_FSYNC=always`, which survives a host crash, p50 was about 13 ms. Both runs were on a single Apple M4 with the load generator and Postgres on the same box, so treat them as a floor for that machine. See `docs/benchmarks.md`.

## How do affiliates get paid?

bunfiiliate builds the payout batch and a bank CSV. You can pay by hand and mark the batch paid, or send through PayPal Payouts or the Wise adapter. The Wise adapter has not been run against Wise's sandbox yet.

## Which license?

AGPL-3.0. It is a clean-room implementation: no code, schema text or copy from any closed-source project.

## Can I trust the ledger?

Postgres enforces it. Updates and deletes on `ledger_entries` are rejected by trigger, and every transaction must sum to zero or the commit fails. Read [Ledger](#/ledger).
