//! Pure domain types and money math. No IO.

pub mod commission;
pub mod ids;
pub mod ledger;
pub mod money;

pub use commission::{CommissionError, CommissionKind, CommissionPlan, Duration};
pub use ids::{AffiliateId, ConversionId, PayoutId, ProgramId, TxnId};
pub use ledger::{Account, LedgerTxn, Posting, Unbalanced};
pub use money::{Bps, Currency, Money, MoneyError};
