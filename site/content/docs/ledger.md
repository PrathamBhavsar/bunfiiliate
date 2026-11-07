---
title: Ledger
group: Reference
order: 6
description: Double-entry postings, commission rounding and duration windows.
---

# Ledger and commission math

All money is integer minor units (`Money{cents: i64, currency}`). No floats anywhere.

## Postings

A ledger transaction is a set of signed postings that sums to zero per currency. Debit is positive, credit is negative. Constructing an unbalanced one returns an error, and Postgres enforces the same rule with a deferrable constraint trigger. Triggers also reject `UPDATE` and `DELETE` on `ledger_entries`.

Accounts: `Expense`, `AffPending`, `AffPayable`, `Cash`.

| Event | Debit | Credit |
| --- | --- | --- |
| Conversion | Expense | AffPending |
| Approve | AffPending | AffPayable |
| Reject or refund before approval | AffPending | Expense |
| Chargeback or refund after approval | AffPayable | Expense |
| Payout paid | AffPayable | Cash |

A chargeback after approval may drive a balance negative. The clawback rolls into the next payout. Balances are always derived from posted transactions, never stored.

## Commission rounding

For a percent plan:

```text
commission = floor((amount_cents * rate_bps + 5000) / 10000)
```

computed in `i128`. That is round-half-up on the exact value, within half a cent and never above it. Flat plans pay the fixed amount unrounded. Negative amounts are rejected: refunds are reversals in the ledger, not negative commissions.

At 10% (1000 bps): 5c becomes 1c, 4c becomes 0c. At 15%, 10c becomes 2c.

## Duration window

`months_since_attribution` counts whole months since the customer was attributed, with 0 being the first month. Outside the window the commission is zero, not an error.

| Duration | Earns when |
| --- | --- |
| `one_month` | months is 0 |
| `x_months(n)` | months is below n |
| `lifetime` | always |

## Plan precedence

Affiliate override first, then the conversion-type plan, then the program default.
