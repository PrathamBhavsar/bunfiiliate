use std::fmt;

use crate::ledger::{Account, LedgerTxn, Posting};
use crate::money::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConversionStatus {
    Pending,
    Approved,
    Rejected,
    Reversed,
    Paid,
}

/// Events that post carry the commission amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionEvent {
    Approve(Money),
    /// Reject or refund before approval.
    Reject(Money),
    /// Chargeback or refund after approval, paid out or not.
    Reverse(Money),
    /// Posts nothing: the cash movement belongs to the payout.
    MarkPaid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PayoutStatus {
    Draft,
    Approved,
    Paid,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayoutEvent {
    Approve,
    MarkPaid(Money),
    Cancel,
}

/// The event is not allowed from this status, or carries a negative amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition;

impl fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid state transition")
    }
}

impl std::error::Error for InvalidTransition {}

/// Debit `from`, credit `to`. Debit is positive, credit negative.
fn transfer(from: Account, to: Account, amount: Money) -> Result<LedgerTxn, InvalidTransition> {
    if amount.cents < 0 {
        return Err(InvalidTransition);
    }
    LedgerTxn::new(vec![
        Posting {
            account: from,
            amount,
        },
        Posting {
            account: to,
            amount: Money::new(-amount.cents, amount.currency),
        },
    ])
    .map_err(|_| InvalidTransition)
}

/// Posting for a new conversion, which starts out `Pending`.
pub fn conversion_created(commission: Money) -> Result<LedgerTxn, InvalidTransition> {
    transfer(Account::Expense, Account::AffPending, commission)
}

impl ConversionStatus {
    pub fn transition(
        self,
        ev: ConversionEvent,
    ) -> Result<(ConversionStatus, Option<LedgerTxn>), InvalidTransition> {
        use ConversionEvent as E;
        use ConversionStatus as S;
        match (self, ev) {
            (S::Pending, E::Approve(m)) => Ok((
                S::Approved,
                Some(transfer(Account::AffPending, Account::AffPayable, m)?),
            )),
            (S::Pending, E::Reject(m)) => Ok((
                S::Rejected,
                Some(transfer(Account::AffPending, Account::Expense, m)?),
            )),
            // After payout the debit leaves payable negative: a clawback netted against the
            // affiliate's next payable.
            (S::Approved | S::Paid, E::Reverse(m)) => Ok((
                S::Reversed,
                Some(transfer(Account::AffPayable, Account::Expense, m)?),
            )),
            (S::Approved, E::MarkPaid) => Ok((S::Paid, None)),
            _ => Err(InvalidTransition),
        }
    }
}

impl PayoutStatus {
    pub fn transition(
        self,
        ev: PayoutEvent,
    ) -> Result<(PayoutStatus, Option<LedgerTxn>), InvalidTransition> {
        use PayoutEvent as E;
        use PayoutStatus as S;
        match (self, ev) {
            (S::Draft, E::Approve) => Ok((S::Approved, None)),
            (S::Draft, E::Cancel) => Ok((S::Cancelled, None)),
            (S::Approved, E::MarkPaid(m)) => Ok((
                S::Paid,
                Some(transfer(Account::AffPayable, Account::Cash, m)?),
            )),
            _ => Err(InvalidTransition),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;
    use proptest::prelude::*;

    fn usd() -> Currency {
        Currency::new("USD").unwrap()
    }

    fn money(cents: i64) -> Money {
        Money::new(cents, usd())
    }

    fn legs(t: &LedgerTxn) -> Vec<(Account, i64)> {
        t.postings()
            .iter()
            .map(|p| (p.account, p.amount.cents))
            .collect()
    }

    const CONV_STATUSES: [ConversionStatus; 5] = [
        ConversionStatus::Pending,
        ConversionStatus::Approved,
        ConversionStatus::Rejected,
        ConversionStatus::Reversed,
        ConversionStatus::Paid,
    ];

    fn conv_events(m: Money) -> [ConversionEvent; 4] {
        [
            ConversionEvent::Approve(m),
            ConversionEvent::Reject(m),
            ConversionEvent::Reverse(m),
            ConversionEvent::MarkPaid,
        ]
    }

    #[test]
    fn conversion_postings() {
        let m = money(500);
        assert_eq!(
            legs(&conversion_created(m).unwrap()),
            [(Account::Expense, 500), (Account::AffPending, -500)]
        );
        let (s, t) = ConversionStatus::Pending
            .transition(ConversionEvent::Approve(m))
            .unwrap();
        assert_eq!(s, ConversionStatus::Approved);
        assert_eq!(
            legs(&t.unwrap()),
            [(Account::AffPending, 500), (Account::AffPayable, -500)]
        );
        let (s, t) = ConversionStatus::Pending
            .transition(ConversionEvent::Reject(m))
            .unwrap();
        assert_eq!(s, ConversionStatus::Rejected);
        assert_eq!(
            legs(&t.unwrap()),
            [(Account::AffPending, 500), (Account::Expense, -500)]
        );
        let (s, t) = ConversionStatus::Approved
            .transition(ConversionEvent::Reverse(m))
            .unwrap();
        assert_eq!(s, ConversionStatus::Reversed);
        assert_eq!(
            legs(&t.unwrap()),
            [(Account::AffPayable, 500), (Account::Expense, -500)]
        );
        let (s, t) = ConversionStatus::Paid
            .transition(ConversionEvent::Reverse(m))
            .unwrap();
        assert_eq!(s, ConversionStatus::Reversed);
        assert_eq!(
            legs(&t.unwrap()),
            [(Account::AffPayable, 500), (Account::Expense, -500)]
        );
        let (s, t) = ConversionStatus::Approved
            .transition(ConversionEvent::MarkPaid)
            .unwrap();
        assert_eq!((s, t), (ConversionStatus::Paid, None));
    }

    #[test]
    fn payout_postings() {
        let (s, t) = PayoutStatus::Draft
            .transition(PayoutEvent::Approve)
            .unwrap();
        assert_eq!((s, t), (PayoutStatus::Approved, None));
        let (s, t) = PayoutStatus::Draft.transition(PayoutEvent::Cancel).unwrap();
        assert_eq!((s, t), (PayoutStatus::Cancelled, None));
        let (s, t) = PayoutStatus::Approved
            .transition(PayoutEvent::MarkPaid(money(700)))
            .unwrap();
        assert_eq!(s, PayoutStatus::Paid);
        assert_eq!(
            legs(&t.unwrap()),
            [(Account::AffPayable, 700), (Account::Cash, -700)]
        );
    }

    #[test]
    fn conversion_table_is_exact() {
        use ConversionStatus as S;
        let valid = [
            (S::Pending, 0, S::Approved),
            (S::Pending, 1, S::Rejected),
            (S::Approved, 2, S::Reversed),
            (S::Approved, 3, S::Paid),
            (S::Paid, 2, S::Reversed),
        ];
        for from in CONV_STATUSES {
            for (i, ev) in conv_events(money(1)).into_iter().enumerate() {
                let want = valid.iter().find(|(f, e, _)| *f == from && *e == i);
                match (from.transition(ev), want) {
                    (Ok((to, _)), Some((_, _, w))) => assert_eq!(to, *w),
                    (Err(InvalidTransition), None) => {}
                    (got, _) => panic!("{from:?} + {ev:?}: {got:?}"),
                }
            }
        }
    }

    #[test]
    fn payout_table_is_exact() {
        use PayoutStatus as S;
        let events = [
            PayoutEvent::Approve,
            PayoutEvent::MarkPaid(money(1)),
            PayoutEvent::Cancel,
        ];
        let valid = [
            (S::Draft, 0, S::Approved),
            (S::Draft, 2, S::Cancelled),
            (S::Approved, 1, S::Paid),
        ];
        for from in [S::Draft, S::Approved, S::Paid, S::Cancelled] {
            for (i, ev) in events.into_iter().enumerate() {
                let want = valid.iter().find(|(f, e, _)| *f == from && *e == i);
                match (from.transition(ev), want) {
                    (Ok((to, _)), Some((_, _, w))) => assert_eq!(to, *w),
                    (Err(InvalidTransition), None) => {}
                    (got, _) => panic!("{from:?} + {ev:?}: {got:?}"),
                }
            }
        }
    }

    #[test]
    fn negative_amount_is_invalid() {
        assert_eq!(conversion_created(money(-1)), Err(InvalidTransition));
        assert_eq!(
            ConversionStatus::Pending.transition(ConversionEvent::Approve(money(-1))),
            Err(InvalidTransition)
        );
        assert_eq!(
            PayoutStatus::Approved.transition(PayoutEvent::MarkPaid(money(i64::MIN))),
            Err(InvalidTransition)
        );
    }

    /// One random step against a simulated book of conversions and payouts.
    #[derive(Debug, Clone)]
    enum Op {
        Create(i64),
        Conv(usize, u8),
        NewPayout(i64),
        Payout(usize, u8),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..1_000_000_000i64).prop_map(Op::Create),
            (any::<usize>(), 0..4u8).prop_map(|(i, e)| Op::Conv(i, e)),
            (0..1_000_000_000i64).prop_map(Op::NewPayout),
            (any::<usize>(), 0..3u8).prop_map(|(i, e)| Op::Payout(i, e)),
        ]
    }

    /// Credit-normal balance of `account`: credits minus debits.
    fn liability(book: &[LedgerTxn], account: Account) -> i64 {
        book.iter()
            .flat_map(|t| t.postings())
            .filter(|p| p.account == account)
            .map(|p| -p.amount.cents)
            .sum()
    }

    proptest! {
        // Random event sequences, valid or not: invalid ones return Err and change
        // nothing, valid ones post balanced txns, and the books always reconcile.
        #[test]
        fn random_sequences_keep_ledger_consistent(ops in prop::collection::vec(op(), 0..60)) {
            let mut book: Vec<LedgerTxn> = Vec::new();
            let mut convs: Vec<(ConversionStatus, i64)> = Vec::new();
            let mut payouts: Vec<(PayoutStatus, i64)> = Vec::new();
            let mut paid_out = 0i64;

            for op in ops {
                let txn = match op {
                    Op::Create(c) => {
                        convs.push((ConversionStatus::Pending, c));
                        Some(conversion_created(money(c)).unwrap())
                    }
                    Op::NewPayout(c) => {
                        payouts.push((PayoutStatus::Draft, c));
                        None
                    }
                    Op::Conv(i, e) if !convs.is_empty() => {
                        let slot = i % convs.len();
                        let (status, c) = convs[slot];
                        let ev = conv_events(money(c))[usize::from(e)];
                        match status.transition(ev) {
                            Ok((next, t)) => {
                                convs[slot].0 = next;
                                t
                            }
                            Err(InvalidTransition) => None,
                        }
                    }
                    Op::Payout(i, e) if !payouts.is_empty() => {
                        let slot = i % payouts.len();
                        let (status, c) = payouts[slot];
                        let ev = [
                            PayoutEvent::Approve,
                            PayoutEvent::MarkPaid(money(c)),
                            PayoutEvent::Cancel,
                        ][usize::from(e)];
                        match status.transition(ev) {
                            Ok((next, t)) => {
                                if next == PayoutStatus::Paid {
                                    paid_out += c;
                                }
                                payouts[slot].0 = next;
                                t
                            }
                            Err(InvalidTransition) => None,
                        }
                    }
                    _ => None,
                };
                if let Some(t) = txn {
                    let sum: i64 = t.postings().iter().map(|p| p.amount.cents).sum();
                    prop_assert_eq!(sum, 0);
                    book.push(t);
                }
            }

            let live: i64 = convs
                .iter()
                .filter(|(s, _)| !matches!(s, ConversionStatus::Rejected | ConversionStatus::Reversed))
                .map(|(_, c)| c)
                .sum();
            let pending = liability(&book, Account::AffPending);
            let payable = liability(&book, Account::AffPayable);
            prop_assert_eq!(pending + payable, live - paid_out);
            prop_assert_eq!(
                pending,
                convs.iter().filter(|(s, _)| *s == ConversionStatus::Pending).map(|(_, c)| c).sum::<i64>()
            );
            prop_assert_eq!(liability(&book, Account::Cash), paid_out);
        }
    }
}
