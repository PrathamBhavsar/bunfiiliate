---
type: Architecture
title: Ledger and State Machines
description: Double-entry append-only ledger postings and conversion/payout transitions.
tags: [ledger, money, state-machine, proptest]
timestamp: 2026-10-26
---

# Ledger and State Machines

Money = integer cents + currency. Bps math, round-half-up. Balances derived from `ledger_entries` (append-only, trigger blocks UPDATE/DELETE, deferred constraint: sum per txn = 0).

## Postings
| Event | Debit | Credit |
| :--- | :--- | :--- |
| Conversion | Expense | AffPending |
| Approve | AffPending | AffPayable |
| Reject/refund pre-approval | AffPending | Expense |
| Chargeback post-approval | AffPayable | Expense (may go negative; clawback next payout) |
| Payout paid | AffPayable | Cash |

## State machines
Enum + transition table (`transition(self, ev) -> Result<(Status, LedgerTxn), InvalidTransition>`), not typestate generics: entities load from DB at runtime.
Conversion: pending -> approved -> paid; pending -> rejected; approved -> reversed.
Payout: draft -> approved -> paid; draft -> cancelled.

## Invariants (proptest)
Any valid event sequence keeps every txn balanced and `payable + pending = live commissions - paid`.

See [Tracker Hot Path](/architecture/tracker-hot-path.md).
