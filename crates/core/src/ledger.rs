use std::fmt;

use crate::money::{Currency, Money};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Account {
    Expense,
    AffPending,
    AffPayable,
    Cash,
}

/// One leg of a transaction. Signed: debit is positive, credit is negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Posting {
    pub account: Account,
    pub amount: Money,
}

/// Postings do not sum to zero in every currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unbalanced;

impl fmt::Display for Unbalanced {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ledger postings do not balance")
    }
}

impl std::error::Error for Unbalanced {}

/// A set of postings that sums to zero per currency. Only constructible balanced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerTxn {
    postings: Vec<Posting>,
}

impl LedgerTxn {
    pub fn new(postings: Vec<Posting>) -> Result<Self, Unbalanced> {
        let mut sums: Vec<(Currency, i128)> = Vec::new();
        for p in &postings {
            let m = p.amount;
            // i128 cannot overflow when summing i64 legs.
            match sums.iter_mut().find(|(c, _)| *c == m.currency) {
                Some((_, s)) => *s += i128::from(m.cents),
                None => sums.push((m.currency, i128::from(m.cents))),
            }
        }
        if sums.iter().all(|(_, s)| *s == 0) {
            Ok(Self { postings })
        } else {
            Err(Unbalanced)
        }
    }

    pub fn postings(&self) -> &[Posting] {
        &self.postings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn cur(i: usize) -> Currency {
        Currency::new(["USD", "EUR", "GBP"][i]).unwrap()
    }

    fn posting() -> impl Strategy<Value = Posting> {
        let account = prop_oneof![
            Just(Account::Expense),
            Just(Account::AffPending),
            Just(Account::AffPayable),
            Just(Account::Cash),
        ];
        (account, any::<i64>(), 0..3usize).prop_map(|(account, cents, c)| Posting {
            account,
            amount: Money::new(cents, cur(c)),
        })
    }

    #[test]
    fn conversion_posting_balances() {
        let usd = cur(0);
        let t = LedgerTxn::new(vec![
            Posting {
                account: Account::Expense,
                amount: Money::new(500, usd),
            },
            Posting {
                account: Account::AffPending,
                amount: Money::new(-500, usd),
            },
        ]);
        assert!(t.is_ok());
        let off = LedgerTxn::new(vec![Posting {
            account: Account::Expense,
            amount: Money::new(500, usd),
        }]);
        assert_eq!(off, Err(Unbalanced));
    }

    proptest! {
        // Any posting set that is balanced by construction is accepted, including
        // extreme amounts; the balancing leg is added in wider arithmetic.
        #[test]
        fn balanced_by_construction_is_accepted(
            legs in prop::collection::vec(posting(), 0..8),
        ) {
            let mut postings = legs.clone();
            for c in 0..3 {
                let sum: i128 = legs.iter()
                    .filter(|p| p.amount.currency == cur(c))
                    .map(|p| i128::from(p.amount.cents))
                    .sum();
                // Spread the balancing amount over i64-sized legs.
                let mut rest = -sum;
                while rest != 0 {
                    let step = rest.clamp(i128::from(i64::MIN), i128::from(i64::MAX));
                    postings.push(Posting {
                        account: Account::Cash,
                        amount: Money::new(step as i64, cur(c)),
                    });
                    rest -= step;
                }
            }
            prop_assert!(LedgerTxn::new(postings).is_ok());
        }

        // Never panics on overflow; accepts exactly when every per-currency sum is 0.
        #[test]
        fn accepts_iff_sums_zero(postings in prop::collection::vec(posting(), 0..8)) {
            let zero = (0..3).all(|c| {
                postings.iter()
                    .filter(|p| p.amount.currency == cur(c))
                    .map(|p| i128::from(p.amount.cents))
                    .sum::<i128>() == 0
            });
            prop_assert_eq!(LedgerTxn::new(postings).is_ok(), zero);
        }
    }
}
