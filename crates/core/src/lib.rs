//! Pure domain types and money math. No IO.

pub mod attribution;
pub mod billing;
pub mod commission;
pub mod fraud;
pub mod ids;
pub mod ledger;
pub mod money;
pub mod payout_provider;
pub mod secrets;
pub mod state;
pub mod throttle;
pub mod totp;
pub mod webhook_sign;

pub use attribution::{Attribution, FirstTouch, LastTouch, Touch};
pub use commission::{CommissionError, CommissionKind, CommissionPlan, Duration};
pub use fraud::{
    BotUa, ClickFacts, Dedupe, FraudRule, IpVelocity, Verdict, default_rules, first_flag,
};
pub use ids::{AffiliateId, ConversionId, PayoutId, ProgramId, TxnId};
pub use ledger::{Account, LedgerTxn, Posting, Unbalanced};
pub use money::{Bps, Currency, Money, MoneyError};
pub use payout_provider::{
    LineEvent, LineState, PayoutDestination, PayoutError, PayoutReceipt, Rail, RailStatus,
};
pub use state::{
    ConversionEvent, ConversionStatus, InvalidTransition, PayoutEvent, PayoutStatus,
    conversion_created,
};
