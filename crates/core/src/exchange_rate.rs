//! Exchange-rate math. Pure: no IO, no floats.
//!
//! Rates are fixed-point with 8 decimals (`numeric(18,8)` in the DB). Only "units of currency
//! per 1 USD" rates are stored; any other pair is triangulated through USD, so
//! `EUR -> GBP` is `cents * gbp_per_usd / eur_per_usd` with a single rounding step.
//! Conversion to integer cents rounds half-even (banker's rounding).

use std::fmt;

const SCALE: i128 = 100_000_000;
const DECIMALS: usize = 8;

/// Strictly positive fixed-point rate, scaled by 1e8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rate(i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateError {
    /// Not a positive decimal with at most 8 fractional digits that fits `numeric(18,8)`.
    Invalid,
    Overflow,
}

impl fmt::Display for RateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid exchange rate",
            Self::Overflow => "exchange conversion overflow",
        })
    }
}

impl std::error::Error for RateError {}

impl Rate {
    /// 1.0, the rate of USD against itself.
    pub const ONE: Self = Self(SCALE as i64);

    /// From the scaled integer (`1.5` is `150_000_000`). Must be positive and below 1e10.
    pub fn from_scaled(scaled: i64) -> Result<Self, RateError> {
        // numeric(18,8): at most 18 digits in all.
        if scaled <= 0 || scaled > 999_999_999_999_999_999 {
            return Err(RateError::Invalid);
        }
        Ok(Self(scaled))
    }

    pub fn scaled(self) -> i64 {
        self.0
    }

    /// Parse plain decimal text (`"0.89566"`). No sign, no exponent, at most 8 decimals.
    pub fn parse(s: &str) -> Result<Self, RateError> {
        let (int, frac) = s.split_once('.').unwrap_or((s, ""));
        let digits = |t: &str| t.bytes().all(|b| b.is_ascii_digit());
        if int.is_empty()
            || (s.ends_with('.'))
            || !digits(int)
            || !digits(frac)
            || frac.len() > DECIMALS
        {
            return Err(RateError::Invalid);
        }
        let int: i128 = int.parse().map_err(|_| RateError::Invalid)?;
        let mut frac_scaled: i128 = if frac.is_empty() {
            0
        } else {
            frac.parse().map_err(|_| RateError::Invalid)?
        };
        for _ in frac.len()..DECIMALS {
            frac_scaled *= 10;
        }
        let scaled = int
            .checked_mul(SCALE)
            .and_then(|v| v.checked_add(frac_scaled))
            .ok_or(RateError::Invalid)?;
        let scaled = i64::try_from(scaled).map_err(|_| RateError::Invalid)?;
        Self::from_scaled(scaled)
    }
}

/// `num / den` rounded half-even. `den` must be positive.
fn div_half_even(num: i128, den: i128) -> i128 {
    let (q, r) = (num.div_euclid(den), num.rem_euclid(den));
    match (r * 2).cmp(&den) {
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Greater => q + 1,
        std::cmp::Ordering::Equal => {
            if q % 2 == 0 {
                q
            } else {
                q + 1
            }
        }
    }
}

/// Convert `cents` of a currency worth `from_per_usd` units per USD into one worth
/// `to_per_usd`, rounding half-even to whole cents. USD itself is [`Rate::ONE`].
pub fn convert_cents(cents: i64, from_per_usd: Rate, to_per_usd: Rate) -> Result<i64, RateError> {
    let num = i128::from(cents) * i128::from(to_per_usd.0);
    let out = div_half_even(num, i128::from(from_per_usd.0));
    i64::try_from(out).map_err(|_| RateError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rate {
        Rate::parse(s).unwrap()
    }

    #[test]
    fn parse_accepts_plain_decimals_only() {
        assert_eq!(r("0.89566").scaled(), 89_566_000);
        assert_eq!(r("1").scaled(), 100_000_000);
        assert_eq!(r("151.12345678").scaled(), 15_112_345_678);
        for bad in [
            "",
            ".5",
            "-1",
            "1e-5",
            "0",
            "0.000000000",
            "1.123456789",
            "a",
            "1.",
            "1,5",
        ] {
            assert!(Rate::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_values_outside_numeric_18_8() {
        assert!(Rate::parse("9999999999.99999999").is_ok());
        assert!(Rate::parse("10000000000").is_err());
    }

    #[test]
    fn half_even_rounds_ties_to_even() {
        // 0.5 USD cents per ... rate 0.5: 1 cent -> 0.5 -> 0 (even); 3 cents -> 1.5 -> 2; 5 -> 2.5 -> 2.
        let half = r("0.5");
        assert_eq!(convert_cents(1, Rate::ONE, half), Ok(0));
        assert_eq!(convert_cents(3, Rate::ONE, half), Ok(2));
        assert_eq!(convert_cents(5, Rate::ONE, half), Ok(2));
        assert_eq!(convert_cents(7, Rate::ONE, half), Ok(4));
        // Negative amounts (clawbacks) mirror: -1.5 -> -2, -2.5 -> -2.
        assert_eq!(convert_cents(-3, Rate::ONE, half), Ok(-2));
        assert_eq!(convert_cents(-5, Rate::ONE, half), Ok(-2));
        // Not a tie: 0.51 -> 1, 0.49 -> 0.
        assert_eq!(convert_cents(51, Rate::ONE, r("0.01")), Ok(1)); // 0.51 cents
        assert_eq!(convert_cents(49, Rate::ONE, r("0.01")), Ok(0));
    }

    #[test]
    fn usd_to_eur_and_back() {
        let eur = r("0.89566"); // EUR per USD
        assert_eq!(convert_cents(10_000, Rate::ONE, eur), Ok(8_957)); // 8956.6 -> 8957
        assert_eq!(convert_cents(8_957, eur, Rate::ONE), Ok(10_000)); // 10000.45 -> 10000
    }

    #[test]
    fn cross_rate_triangulates_through_usd_with_one_rounding() {
        let (eur, gbp) = (r("0.89566"), r("0.74711"));
        // 10000 * 0.74711 / 0.89566 = 8341.4... -> 8341
        assert_eq!(convert_cents(10_000, eur, gbp), Ok(8_341));
        // Identity: same currency is exact.
        assert_eq!(convert_cents(12_345, eur, eur), Ok(12_345));
        assert_eq!(convert_cents(i64::MAX, eur, eur), Ok(i64::MAX));
    }

    #[test]
    fn overflow_is_an_error() {
        let big = r("9999999999");
        assert_eq!(
            convert_cents(i64::MAX, Rate::ONE, big),
            Err(RateError::Overflow)
        );
    }
}
