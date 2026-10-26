use std::fmt;

use crate::money::{Bps, Money};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommissionKind {
    /// Share of the order amount, in basis points.
    Percent(Bps),
    /// Fixed cents per qualifying conversion.
    Flat(i64),
}

/// How long after attribution a customer's conversions earn commission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duration {
    OneMonth,
    XMonths(u32),
    Lifetime,
}

impl Duration {
    fn covers(self, months_since_attribution: u32) -> bool {
        match self {
            Self::OneMonth => months_since_attribution < 1,
            Self::XMonths(n) => months_since_attribution < n,
            Self::Lifetime => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommissionError {
    /// Negative order amount or negative flat commission.
    Negative,
    Overflow,
}

impl fmt::Display for CommissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Negative => "negative amount",
            Self::Overflow => "commission overflow",
        })
    }
}

impl std::error::Error for CommissionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommissionPlan {
    pub kind: CommissionKind,
    pub duration: Duration,
}

impl CommissionPlan {
    /// Commission on `amount`, `months_since_attribution` whole months after the
    /// customer was attributed. Zero outside the duration window. Percent plans
    /// use integer bps math, rounded half up (see `docs/ledger.md`).
    pub fn commission_for(
        &self,
        amount: Money,
        months_since_attribution: u32,
    ) -> Result<Money, CommissionError> {
        if amount.cents < 0 {
            return Err(CommissionError::Negative);
        }
        let cents = if !self.duration.covers(months_since_attribution) {
            0
        } else {
            match self.kind {
                CommissionKind::Percent(bps) => {
                    let scaled = i128::from(amount.cents) * i128::from(bps.get());
                    let half_up = (scaled + 5_000) / 10_000;
                    i64::try_from(half_up).map_err(|_| CommissionError::Overflow)?
                }
                CommissionKind::Flat(c) if c < 0 => return Err(CommissionError::Negative),
                CommissionKind::Flat(c) => c,
            }
        };
        Ok(Money::new(cents, amount.currency))
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

    fn percent(bps: u32, duration: Duration) -> CommissionPlan {
        CommissionPlan {
            kind: CommissionKind::Percent(Bps::new(bps).unwrap()),
            duration,
        }
    }

    #[test]
    fn golden_table() {
        // (kind, duration, amount cents, months since attribution, expected cents)
        let p = |b| CommissionKind::Percent(Bps::new(b).unwrap());
        let table = [
            (p(1000), Duration::Lifetime, 10_000, 0, 1_000),
            (p(1000), Duration::Lifetime, 0, 0, 0),
            (p(1000), Duration::Lifetime, 5, 0, 1), // 0.5 -> 1 (half up)
            (p(1000), Duration::Lifetime, 4, 0, 0), // 0.4 -> 0
            (p(1500), Duration::Lifetime, 3, 0, 0), // 0.45 -> 0
            (p(1500), Duration::Lifetime, 10, 0, 2), // 1.5 -> 2
            (p(3333), Duration::Lifetime, 100, 0, 33), // 33.33 -> 33
            (p(3350), Duration::Lifetime, 100, 0, 34), // 33.5 -> 34
            (p(1), Duration::Lifetime, 49_999, 0, 5), // 4.9999 -> 5
            (p(10_000), Duration::Lifetime, 12_345, 0, 12_345),
            (p(0), Duration::Lifetime, 12_345, 0, 0),
            (p(2000), Duration::OneMonth, 1_000, 0, 200),
            (p(2000), Duration::OneMonth, 1_000, 1, 0),
            (p(2000), Duration::XMonths(3), 1_000, 2, 200),
            (p(2000), Duration::XMonths(3), 1_000, 3, 0),
            (p(2000), Duration::XMonths(0), 1_000, 0, 0),
            (p(2000), Duration::Lifetime, 1_000, 999, 200),
            (CommissionKind::Flat(500), Duration::Lifetime, 1_000, 7, 500),
            (CommissionKind::Flat(500), Duration::OneMonth, 1_000, 1, 0),
            (
                p(1000),
                Duration::Lifetime,
                i64::MAX,
                0,
                922_337_203_685_477_581, // i64::MAX / 10 = ...580.7
            ), // 9223372036854775807 / 10
        ];
        for (kind, duration, amount, months, want) in table {
            let plan = CommissionPlan { kind, duration };
            let got = plan.commission_for(Money::new(amount, usd()), months);
            assert_eq!(
                got,
                Ok(Money::new(want, usd())),
                "{kind:?} {duration:?} amount={amount} months={months}"
            );
        }
    }

    #[test]
    fn negatives_rejected() {
        let plan = percent(1000, Duration::Lifetime);
        assert_eq!(
            plan.commission_for(Money::new(-1, usd()), 0),
            Err(CommissionError::Negative)
        );
        let flat = CommissionPlan {
            kind: CommissionKind::Flat(-1),
            duration: Duration::Lifetime,
        };
        assert_eq!(
            flat.commission_for(Money::new(1, usd()), 0),
            Err(CommissionError::Negative)
        );
    }

    fn duration() -> impl Strategy<Value = Duration> {
        prop_oneof![
            Just(Duration::OneMonth),
            any::<u32>().prop_map(Duration::XMonths),
            Just(Duration::Lifetime),
        ]
    }

    proptest! {
        // No panic across the full i64 range, and the result is within half a
        // cent (so within 1 cent) of the exact rational commission.
        #[test]
        fn percent_rounding_within_one_cent(
            amount in 0..=i64::MAX,
            bps in 0..=Bps::MAX,
            months in any::<u32>(),
        ) {
            let plan = percent(bps, Duration::Lifetime);
            let got = plan.commission_for(Money::new(amount, usd()), months).unwrap().cents;
            let diff = (i128::from(got) * 10_000 - i128::from(amount) * i128::from(bps)).abs();
            prop_assert!(diff <= 5_000);
            prop_assert!(got <= amount);
        }

        #[test]
        fn duration_window(
            amount in 0..=i64::MAX,
            bps in 1..=Bps::MAX,
            d in duration(),
            months in any::<u32>(),
        ) {
            let plan = percent(bps, d);
            let got = plan.commission_for(Money::new(amount, usd()), months).unwrap().cents;
            let in_window = match d {
                Duration::OneMonth => months == 0,
                Duration::XMonths(n) => months < n,
                Duration::Lifetime => true,
            };
            if !in_window {
                prop_assert_eq!(got, 0);
            }
            let full = percent(bps, Duration::Lifetime)
                .commission_for(Money::new(amount, usd()), months).unwrap().cents;
            prop_assert_eq!(got, if in_window { full } else { 0 });
        }

        #[test]
        fn flat_never_panics(c in any::<i64>(), amount in any::<i64>(), d in duration(), months in any::<u32>()) {
            let plan = CommissionPlan { kind: CommissionKind::Flat(c), duration: d };
            let _ = plan.commission_for(Money::new(amount, usd()), months);
        }
    }
}
