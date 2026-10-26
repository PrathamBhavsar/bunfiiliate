---
type: Architecture
title: Worker and Ingest Processor
description: api worker mode - inbound event to conversion, attribution, plan resolution, auto-approve tick.
tags: [worker, ingest, attribution, outbox]
timestamp: 2026-10-26
---

# Worker and Ingest Processor

`api worker` or `api serve --with-worker` runs `api::worker::run`: a 1s ingest loop and a 60s approve tick (plain tokio intervals). See [ADR-0002](/decisions/0002-worker-as-api-mode.md).

## Ingest (one event per transaction)
1. Claim the oldest unprocessed `inbound_events` row `FOR UPDATE SKIP LOCKED`.
2. Work inside a savepoint. Success: commit it. Rejected (or integrity/data DB error): roll the savepoint back, set `processed_at` + `error`. Transient DB error: abort, event stays unprocessed.
3. Payload facts per source: generic `{customer, amount_cents, currency?, type?, click?, code?, occurred_at?}`; Stripe `invoice.paid` / `checkout.session.completed` (`data.object`, `metadata.bf_click|bf_ref`); Paddle `transaction.completed` (`data.custom_data.bf_click|bf_ref`). Other types are processed with no conversion. `occurred_at` falls back to `received_at`.
4. Customer known with an affiliate: recurring, credited to that affiliate. Else touches = clicks of the visitor behind `click` within `cookie_days` before `occurred_at` (fallback: bare `code`); `bf_core::{FirstTouch, LastTouch}` per `programs.attribution_model` picks one. No touch = organic, processed, nothing recorded. Only active affiliates enrolled in the program can be credited (touch candidates are filtered; recurring is re-checked).
5. Plan: enrollment `plan_override_id`, else the conversion type's plan, else `programs.default_plan_id`. `months_since_attribution` = whole months from `customers.attributed_at` (= first conversion `occurred_at`) to `occurred_at`; outside the window commission is 0 (conversion still recorded).
6. Insert conversion (`ON CONFLICT (program_id, external_id) DO NOTHING`; conflict = replay, stop), post `conversion_created` ledger txn (skipped when 0 cents), outbox `conversion.created`.

## Auto-approve tick
`pg_try_advisory_xact_lock`, then pending conversions of `auto_approve` programs with `occurred_at <= now() - hold_days`, active enrolled affiliates only (suspended stay pending for manual review), batch 100 `FOR UPDATE SKIP LOCKED`: `ConversionStatus::transition(Approve)`, ledger txn, outbox `conversion.approved`.

Outbox dispatch (webhooks, email, notifications) is [Ops Routes](/architecture/ops.md); the worker also runs its two 1s ticks. Not here yet: fraud holds (T40), reversals/payouts (T18).

See [Ledger](/architecture/ledger.md).
