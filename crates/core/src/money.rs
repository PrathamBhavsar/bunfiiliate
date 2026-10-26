use std::fmt;

/// ISO 4217-style code: three uppercase ASCII letters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Currency([u8; 3]);

impl Currency {
    pub fn new(code: &str) -> Option<Self> {
        let b: [u8; 3] = code.as_bytes().try_into().ok()?;
        b.iter().all(u8::is_ascii_uppercase).then_some(Self(b))
    }

    pub fn as_str(&self) -> &str {
        // Invariant: all bytes are ASCII uppercase.
        std::str::from_utf8(&self.0).unwrap_or("???")
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    CurrencyMismatch,
    Overflow,
}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CurrencyMismatch => "currency mismatch",
            Self::Overflow => "money overflow",
        })
    }
}

impl std::error::Error for MoneyError {}

/// Signed amount in minor units (cents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Money {
    pub cents: i64,
    pub currency: Currency,
}

impl Money {
    pub fn new(cents: i64, currency: Currency) -> Self {
        Self { cents, currency }
    }

    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch);
        }
        let cents = self
            .cents
            .checked_add(other.cents)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self::new(cents, self.currency))
    }

    pub fn checked_neg(self) -> Result<Self, MoneyError> {
        let cents = self.cents.checked_neg().ok_or(MoneyError::Overflow)?;
        Ok(Self::new(cents, self.currency))
    }
}

/// Basis points, 0..=10_000 (100%).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Bps(u32);

impl Bps {
    pub const MAX: u32 = 10_000;

    pub fn new(bps: u32) -> Option<Self> {
        (bps <= Self::MAX).then_some(Self(bps))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}
