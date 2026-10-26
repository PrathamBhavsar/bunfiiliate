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

## Reversals (T18)
Refund/chargeback events are handled before conversion parsing in `handle` (`worker::reversal`, `reverse`): generic `{type: refund|chargeback, conversion: <original external_id>}`; Stripe `charge.refunded` / `charge.dispute.created`; Paddle `adjustment.created` with `data.action` refund/chargeback. Stripe/Paddle find the original through the earlier conversion event's payload (`data.object.{id,charge,payment_intent}` / `data.id` vs the refund's ids). Always the full commission (no partial refunds). Conversion is locked, then: pending -> `Reject` (rejected); approved or paid -> `Reverse` (reversed, `AffPayable` debit, goes negative after payout). Already rejected/reversed or unknown (organic) = no-op, so replays are safe. Outbox `conversion.reversed`. A conversion claimed by a draft/approved payout shrinks that payout by its commission.

## Status changes (`worker::settle`)
`settle(conn, &Locked, Act::{Approve,Reject,Reverse}, reason)` is the only writer of `conversions.status` after ingest: runs `ConversionStatus::transition`, updates the status and `approved_at`/`reversed_at`, posts the balanced txn, shrinks an open payout, writes outbox `conversion.approved|reversed`. Callers lock the row first (`LOCKED_COLS`, `FOR UPDATE OF c`): `reverse` (refund/chargeback events), `approve_batch` (tick) and `money::run_action` (admin routes, reason `manual`). A transition the table forbids is `Fail::Rejected` (admin route: `409`).

## Payouts (`api::payouts`, `bf_db::payouts`; routes in [API Routes](/architecture/api-routes.md))
- `create_batch(program, created_by) -> Option<batch>`: locks the program row (`FOR NO KEY UPDATE`), pays every active affiliate whose payable minus their open (draft/approved) payouts is > 0 and >= `payout_min_cents`. Payout = that whole amount, so clawbacks net automatically; the affiliate's approved unclaimed conversions get `conversions.payout_id` (migration 0018). Nothing is posted yet.
- `approve_batch`, `cancel_batch` (draft only, releases the conversions), `mark_paid` (approved only: per payout `AffPayable -> Cash` txn with `payout_id`, `payouts.ledger_txn_id`, claimed approved conversions become `paid`). Invalid transition = `AppError::Conflict`, unknown batch = `NotFound`; a second `mark_paid` is rejected and posts nothing.
- `batch_csv` / pure `export_csv`: `email,amount,currency`, amount decimal with two places (assumes 2-digit currencies), email = `payout_email` else the user's email.
- Edge: a reversal can shrink an open payout to <= 0 when a clawback was netted into it; `mark_paid` then returns Conflict (negative amount) and the operator cancels the batch.

Not here yet: fraud holds (T40), outbox dispatch (T24).
Outbox dispatch (webhooks, email, notifications) is [Ops Routes](/architecture/ops.md); the worker also runs its two 1s ticks. Not here yet: fraud holds (T40), reversals/payouts (T18).
## Fraud holds (T40)
Ingest looks up the customer's `first_click_id`; if that click has `clicks.fraud_reason` the conversion is inserted with `hold_reason = 'click_flagged:<reason>'`. It is posted to the ledger like any pending conversion (so reversals and balances work) but `approve_batch` requires `hold_reason IS NULL`. `worker::release_hold(pool, id) -> bool` clears the hold (pending + held only); the conversion then follows the normal hold-days auto-approve. Manual approve routes (T22) must also refuse held conversions until released.

Not here yet: outbox dispatch (T24).

See [Ledger](/architecture/ledger.md).
