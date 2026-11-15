//! HMAC-SHA256 signing of inbound conversion deliveries. The tracker verifies with [`mac`];
//! the API's webhook tester signs with [`sign`], so both sides share one construction.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Which delivery layout to sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `X-Bf-Signature`, signed `<ts>.<idempotency key>.<body>`.
    Generic,
    /// `Stripe-Signature: t=<ts>,v1=<hex>`, signed `<ts>.<body>`.
    Stripe,
    /// `Paddle-Signature: ts=<ts>;h1=<hex>`, signed `<ts>:<body>`.
    Paddle,
}

/// A keyed MAC over the concatenation of `parts`.
pub fn mac(secret: &str, parts: &[&[u8]]) -> Hmac<Sha256> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    for p in parts {
        mac.update(p);
    }
    mac
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The headers a correctly signed delivery carries. `key` is the idempotency key (generic only).
pub fn sign(
    layout: Layout,
    secret: &str,
    key: &str,
    body: &[u8],
    now: i64,
) -> Vec<(&'static str, String)> {
    let ts = now.to_string();
    let tag = |glue: &[u8], key: Option<&str>| {
        let m = match key {
            Some(k) => mac(secret, &[ts.as_bytes(), b".", k.as_bytes(), b".", body]),
            None => mac(secret, &[ts.as_bytes(), glue, body]),
        };
        hex(&m.finalize().into_bytes())
    };
    match layout {
        Layout::Generic => vec![
            ("x-bf-signature", tag(b".", Some(key))),
            ("x-bf-timestamp", ts.clone()),
            ("idempotency-key", key.to_owned()),
        ],
        Layout::Stripe => vec![("stripe-signature", format!("t={ts},v1={}", tag(b".", None)))],
        Layout::Paddle => vec![(
            "paddle-signature",
            format!("ts={ts};h1={}", tag(b":", None)),
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Same independently computed vectors the tracker pins (Python hmac).
    #[test]
    fn matches_the_documented_layouts() {
        let body = br#"{"type":"sale","amount_cents":4900}"#;
        let h = sign(
            Layout::Generic,
            "whsec_generic",
            "order-1",
            body,
            1_700_000_000,
        );
        assert_eq!(
            h[0].1,
            "a18d4e38b44b600b8a3cfd6badfd79dea2fa77fba632ecffe72db424d6b4045f"
        );
        let body = br#"{"id":"evt_1NG8Du2eZvKYlo2CUI79vXWy","object":"event","type":"checkout.session.completed"}"#;
        let h = sign(Layout::Stripe, "whsec_test_secret", "", body, 1492774577);
        assert_eq!(
            h[0].1,
            "t=1492774577,v1=ab1e77e4c00401a0f27e4f73afe4d17121b6d3ab9d0e9b18015d7433cfa77495"
        );
        let body = br#"{"event_id":"evt_01hv8wptq8987qeqjtaqfc2g3c","event_type":"transaction.completed"}"#;
        let h = sign(
            Layout::Paddle,
            "pdl_ntfset_test_secret",
            "",
            body,
            1671552777,
        );
        assert_eq!(
            h[0].1,
            "ts=1671552777;h1=3590e53213c1a8c45bdbfd19fc064f8b8556a52fc2e7645fc3c08d97366a0c81"
        );
    }
}
