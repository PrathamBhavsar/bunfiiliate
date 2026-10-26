# Ledger and commission math

All money is integer minor units (`Money{cents:i64, currency}`). No floats anywhere.

## Postings

A `LedgerTxn` is a set of signed `Posting{account, amount}` that sums to zero per currency. Debit is positive, credit is negative. `LedgerTxn::new` returns `Err(Unbalanced)` otherwise, so an unbalanced transaction cannot exist. Sums use `i128`, so extreme `i64` legs cannot overflow or panic.

Accounts: `Expense`, `AffPending`, `AffPayable`, `Cash`.

| Event | Debit | Credit |
|---|---|---|
| Conversion | Expense | AffPending |
| Approve | AffPending | AffPayable |
| Reject/refund before approval | AffPending | Expense |
| Chargeback after approval | AffPayable | Expense (balance may go negative; clawback rolls into next payout) |
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
|---|---|
| `one_month` | months == 0 |
| `x_months(n)` | months < n |
| `lifetime` | always |
