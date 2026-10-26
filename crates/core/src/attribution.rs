use uuid::Uuid;

use crate::ids::AffiliateId;

/// One referral touch: a click (or a bare referral code, `click_id` None) by an affiliate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Touch {
    pub click_id: Option<Uuid>,
    pub affiliate: AffiliateId,
    pub at_ms: i64,
}

/// Picks the touch that gets the credit.
pub trait Attribution {
    fn pick(&self, touches: &[Touch]) -> Option<Touch>;
}

/// Earliest touch wins; ties keep the first listed.
pub struct FirstTouch;

/// Latest touch wins; ties keep the last listed.
pub struct LastTouch;

impl Attribution for FirstTouch {
    fn pick(&self, touches: &[Touch]) -> Option<Touch> {
        touches.iter().min_by_key(|t| t.at_ms).copied()
    }
}

impl Attribution for LastTouch {
    fn pick(&self, touches: &[Touch]) -> Option<Touch> {
        touches.iter().max_by_key(|t| t.at_ms).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(at_ms: i64) -> Touch {
        Touch {
            click_id: Some(Uuid::from_u128(at_ms as u128)),
            affiliate: AffiliateId(Uuid::from_u128(at_ms as u128)),
            at_ms,
        }
    }

    #[test]
    fn first_and_last() {
        let touches = [touch(20), touch(10), touch(30)];
        assert_eq!(FirstTouch.pick(&touches), Some(touch(10)));
        assert_eq!(LastTouch.pick(&touches), Some(touch(30)));
    }

    #[test]
    fn empty_has_no_winner() {
        assert_eq!(FirstTouch.pick(&[]), None);
        assert_eq!(LastTouch.pick(&[]), None);
    }

    #[test]
    fn ties_follow_listing_order() {
        let (a, b) = (
            touch(5),
            Touch {
                at_ms: 5,
                ..touch(6)
            },
        );
        assert_eq!(FirstTouch.pick(&[a, b]), Some(a));
        assert_eq!(LastTouch.pick(&[a, b]), Some(b));
    }
}
