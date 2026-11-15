//! Payout rails: pure types and the per-line dispatch state machine. No IO; the HTTP adapters
//! live in `api::payout_rails`.

use std::fmt;

/// Where a payout line is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rail {
    ManualCsv,
    PaypalPayouts,
    Wise,
    Mock,
}

impl Rail {
    pub fn as_str(self) -> &'static str {
        match self {
            Rail::ManualCsv => "manual_csv",
            Rail::PaypalPayouts => "paypal_payouts",
            Rail::Wise => "wise",
            Rail::Mock => "mock",
        }
    }

    pub fn parse(s: &str) -> Option<Rail> {
        Some(match s {
            "manual_csv" => Rail::ManualCsv,
            "paypal_payouts" => Rail::PaypalPayouts,
            "wise" => Rail::Wise,
            "mock" => Rail::Mock,
            _ => return None,
        })
    }

    /// Rails that need per-workspace credentials.
    pub fn needs_credentials(self) -> bool {
        matches!(self, Rail::PaypalPayouts | Rail::Wise)
    }
}

/// Who gets paid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutDestination {
    pub email: String,
}

/// The provider's reference for a submitted payout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutReceipt {
    pub reference: String,
}

/// What the provider says about a payout. Only `Paid` is final success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailStatus {
    Pending,
    Paid,
    Failed,
}

impl RailStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RailStatus::Pending => "pending",
            RailStatus::Paid => "paid",
            RailStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayoutError {
    /// The provider refused the payout; nothing was sent.
    Rejected(String),
    /// Outcome unknown (timeout, 5xx). Retrying with the same idempotency key is safe.
    Transient(String),
}

impl fmt::Display for PayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayoutError::Rejected(m) => write!(f, "rejected: {m}"),
            PayoutError::Transient(m) => write!(f, "transient: {m}"),
        }
    }
}

impl std::error::Error for PayoutError {}

/// Dispatch state of one payout line on a non-manual rail:
/// `pending -> submitted -> paid | failed`, and `failed -> submitted` on retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineState {
    Pending,
    Submitted,
    Paid,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEvent {
    /// About to call the provider.
    Submit,
    /// Provider confirmed the money moved.
    Confirm,
    /// Provider refused or reported failure.
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadLineTransition;

impl LineState {
    pub fn as_str(self) -> &'static str {
        match self {
            LineState::Pending => "pending",
            LineState::Submitted => "submitted",
            LineState::Paid => "paid",
            LineState::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<LineState> {
        Some(match s {
            "pending" => LineState::Pending,
            "submitted" => LineState::Submitted,
            "paid" => LineState::Paid,
            "failed" => LineState::Failed,
            _ => return None,
        })
    }

    pub fn transition(self, ev: LineEvent) -> Result<LineState, BadLineTransition> {
        use LineEvent as E;
        use LineState as S;
        match (self, ev) {
            // Submitting again is the retry of an unknown outcome; the idempotency key makes it safe.
            (S::Pending | S::Failed | S::Submitted, E::Submit) => Ok(S::Submitted),
            (S::Submitted, E::Confirm) => Ok(S::Paid),
            (S::Submitted, E::Fail) => Ok(S::Failed),
            _ => Err(BadLineTransition),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_round_trips() {
        for r in [Rail::ManualCsv, Rail::PaypalPayouts, Rail::Wise, Rail::Mock] {
            assert_eq!(Rail::parse(r.as_str()), Some(r));
        }
        assert_eq!(Rail::parse("venmo"), None);
        assert!(Rail::Wise.needs_credentials() && !Rail::Mock.needs_credentials());
    }

    #[test]
    fn happy_path_and_retry() {
        let s = LineState::Pending.transition(LineEvent::Submit).unwrap();
        assert_eq!(s, LineState::Submitted);
        assert_eq!(s.transition(LineEvent::Confirm), Ok(LineState::Paid));
        let f = s.transition(LineEvent::Fail).unwrap();
        assert_eq!(f, LineState::Failed);
        assert_eq!(f.transition(LineEvent::Submit), Ok(LineState::Submitted));
    }

    #[test]
    fn money_cannot_move_without_submit_and_paid_is_final() {
        assert!(LineState::Pending.transition(LineEvent::Confirm).is_err());
        assert!(LineState::Failed.transition(LineEvent::Confirm).is_err());
        for ev in [LineEvent::Submit, LineEvent::Confirm, LineEvent::Fail] {
            assert!(LineState::Paid.transition(ev).is_err());
        }
    }

    #[test]
    fn state_text_round_trips() {
        for s in [
            LineState::Pending,
            LineState::Submitted,
            LineState::Paid,
            LineState::Failed,
        ] {
            assert_eq!(LineState::parse(s.as_str()), Some(s));
        }
    }
}
