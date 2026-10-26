# Ledger and commission math

All money is integer minor units (`Money{cents:i64, currency}`). No floats anywhere.

## Postings

A `LedgerTxn` is a set of signed `Posting{account, amount}` that sums to zero per currency. Debit is positive, credit is negative. `LedgerTxn::new` returns `Err(Unbalanced)` otherwise, so an unbalanced transaction cannot exist. Sums use `i128`, so extreme `i64` legs cannot overflow or panic.

Accounts: `Expense`, `AffPending`, `AffPayable`, `Cash`.

| Event | Debit | Credit |
| --- | --- | --- |
| Conversion | Expense | AffPending |
| Approve | AffPending | AffPayable |
| Reject/refund before approval | AffPending | Expense |
| Chargeback or refund after approval, paid out or not | AffPayable | Expense (balance may go negative; clawback rolls into next payout) |
| Payout paid | AffPayable | Cash |

## Commission rounding

`CommissionPlan::commission_for(amount, months_since_attribution)`:

- **Percent**: `commission = floor((amount_cents * rate_bps + 5000) / 10000)`, computed in `i128`. This is round-half-up on the exact value `amount * bps / 10000`. The result is within half a cent of exact and never exceeds `amount` (bps is capped at 10000).
- **Flat**: the fixed `amount_cents` of the plan, unrounded.
- Negative order amounts or negative flat values are rejected (`CommissionError::Negative`). Refunds are reversals in the ledger, not negative commissions.
- A commission that does not fit `i64` returns `CommissionError::Overflow` (unreachable for percent plans).

Examples at 10% (1000 bps): 5c -> 0.5 -> 1c; 4c -> 0.4 -> 0c; 10c at 15% -> 1.5 -> 2c.

## Duration window

`months_since_attribution` is whole months since the customer was attributed (0 = the first month). Outside the window the commission is zero (not an error):

| Duration | Earns when |
| --- | --- |
| `one_month` | months == 0 |
| `x_months(n)` | months < n |
| `lifetime` | always |

## Provider events: dedup, reversals, limits

- **One payment, one commission.** A conversion also carries the provider's business object
  (`provider_ref`, e.g. the Stripe invoice). Stripe sends both `invoice.paid` and
  `checkout.session.completed` for a subscription payment; the second finds the same
  `provider_ref` and creates nothing. Unpaid checkout sessions earn nothing until
  `checkout.session.async_payment_succeeded` arrives.
- **Zero-amount payments** (trial invoices, 100%-off checkouts) create no conversion, so a flat
  plan cannot pay out on a $0 invoice.
- **A refund can overtake its sale.** A reversal that matches no conversion is retried with
  backoff (30 s, 1 m, 2 m, 4 m, 8 m); after that it is dropped as unmatched, which is what an
  organic (non-affiliate) refund looks like. Every conversion that matches is reversed.
- **Partial refunds do not claw back.** The ledger reverses a whole commission or none of it, so
  Stripe `charge.refunded` with `refunded: false` and Paddle adjustments of type `partial` are
  ignored. Prorated reversals are a known gap. Chargebacks and full refunds reverse everything.
- **Payout shrink is clamped at zero.** A payout can be smaller than the conversions it claimed
  (a clawback was netted into it). A later reversal cannot push it below zero; the shortfall
  stays in the affiliate's payable and nets against the next batch. A payout shrunk to zero is
  closed out without moving money and is left out of the CSV.
- **Payout destination is snapshotted** into the payout when the batch is drafted, and the CSV
  reads that copy. `PATCH` accepts `payout_email: null` to clear it, and the address can only
  change while the affiliate is active.
- **Currencies** are limited to ISO 4217 codes with two decimal places (see
  `crates/api/src/programs.rs`), because amounts and the CSV count in hundredths. `cookie_days`
  and `hold_days` are at most 365.
