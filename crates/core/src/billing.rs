//! Billing provider types and pure logic: provider allowlist, webhook signatures, plan limits.
//! The HTTP adapters live in `api`; nothing here does IO.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// The only providers a webhook route or checkout accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Mock,
    LemonSqueezy,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mock" => Some(Self::Mock),
            "lemonsqueezy" => Some(Self::LemonSqueezy),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::LemonSqueezy => "lemonsqueezy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubStatus {
    Active,
    PastDue,
    Canceled,
}

impl SubStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::PastDue => "past_due",
            Self::Canceled => "canceled",
        }
    }

    /// LemonSqueezy subscription status. `None` for a value we do not know.
    pub fn from_lemonsqueezy(s: &str) -> Option<Self> {
        match s {
            "active" | "on_trial" => Some(Self::Active),
            "past_due" | "unpaid" | "paused" => Some(Self::PastDue),
            "cancelled" | "expired" => Some(Self::Canceled),
            _ => None,
        }
    }
}

/// A provider subscription event, already verified and parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionEvent {
    /// Provider-unique id; replays carry the same one.
    pub event_id: String,
    /// From the checkout's `custom_data.workspace_id`.
    pub workspace_id: Uuid,
    pub provider_subscription_id: Option<String>,
    pub status: SubStatus,
    /// From the checkout's `custom_data.plan_key`; absent keeps the current plan.
    pub plan_key: Option<String>,
    /// The provider's `updated_at`; events are applied in this order.
    pub updated_at: DateTime<Utc>,
    pub current_period_end: Option<DateTime<Utc>>,
}

/// Hex HMAC-SHA256 of `body`.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac takes any key");
    m.update(body);
    data_encoding::HEXLOWER.encode(&m.finalize().into_bytes())
}

/// Constant-time check of a hex signature header.
pub fn verify(secret: &str, signature: &str, body: &[u8]) -> bool {
    let want = sign(secret, body);
    want.as_bytes()
        .ct_eq(signature.trim().to_ascii_lowercase().as_bytes())
        .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Programs,
    Affiliates,
    MonthlyClicks,
}

impl Limit {
    pub fn name(self) -> &'static str {
        match self {
            Self::Programs => "max_programs",
            Self::Affiliates => "max_affiliates",
            Self::MonthlyClicks => "max_monthly_clicks",
        }
    }
}

/// `true` if adding one more of `kind` would pass `max`. `None` is unlimited.
pub fn would_exceed(max: Option<i64>, used: i64) -> bool {
    max.is_some_and(|m| used >= m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_allowlist() {
        assert_eq!(Provider::parse("mock"), Some(Provider::Mock));
        assert_eq!(
            Provider::parse("lemonsqueezy"),
            Some(Provider::LemonSqueezy)
        );
        for bad in ["stripe", "", "Mock", "mock/../x", "paddle"] {
            assert_eq!(Provider::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn signature_roundtrip_and_tamper() {
        let sig = sign("s3cret", b"{\"a\":1}");
        assert!(verify("s3cret", &sig, b"{\"a\":1}"));
        assert!(verify("s3cret", &sig.to_uppercase(), b"{\"a\":1}"));
        assert!(!verify("s3cret", &sig, b"{\"a\":2}"));
        assert!(!verify("other", &sig, b"{\"a\":1}"));
        assert!(!verify("s3cret", "", b"{\"a\":1}"));
    }

    #[test]
    fn signature_matches_rfc4231_vector() {
        // RFC 4231 test case 2.
        assert_eq!(
            sign("Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn lemonsqueezy_status_map() {
        use SubStatus::*;
        let cases = [
            ("active", Some(Active)),
            ("on_trial", Some(Active)),
            ("past_due", Some(PastDue)),
            ("unpaid", Some(PastDue)),
            ("paused", Some(PastDue)),
            ("cancelled", Some(Canceled)),
            ("expired", Some(Canceled)),
            ("weird", None),
        ];
        for (s, want) in cases {
            assert_eq!(SubStatus::from_lemonsqueezy(s), want, "{s}");
        }
    }

    #[test]
    fn limit_boundaries() {
        assert!(!would_exceed(None, 1_000_000));
        assert!(!would_exceed(Some(2), 1));
        assert!(would_exceed(Some(2), 2));
        assert!(would_exceed(Some(0), 0));
    }
}
