//! Conversion ingest: signature checks for generic, Stripe and Paddle deliveries.
//! HMAC-SHA256 only (no vendor SDKs); tags are compared in constant time.

use axum::http::HeaderMap;
use bf_core::webhook_sign::mac;
use hmac::Mac;
use serde_json::Value;

use crate::snapshot::ProgramSecrets;

/// Largest allowed gap between a signed timestamp and now.
const TOLERANCE_SECS: i64 = 300;
const MAX_EXTERNAL_ID: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Generic,
    /// `POST /v2/events`: `Bf-Signature: t=<secs>,v1=<hex>[,v1=<hex>]` over `<t>.<key>.<body>`.
    GenericV2,
    Stripe,
    Paddle,
}

#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    Missing,
    Malformed,
    Mismatch,
    Stale,
}

impl Source {
    /// The `event_source` enum label.
    pub fn name(self) -> &'static str {
        match self {
            Source::Generic | Source::GenericV2 => "generic",
            Source::Stripe => "stripe",
            Source::Paddle => "paddle",
        }
    }

    /// Status for an accepted delivery: generic acks with 202, webhooks with 200.
    pub fn accepted(self) -> axum::http::StatusCode {
        match self {
            Source::Generic | Source::GenericV2 => axum::http::StatusCode::ACCEPTED,
            Source::Stripe | Source::Paddle => axum::http::StatusCode::OK,
        }
    }

    pub fn secret(self, p: &ProgramSecrets) -> Option<&str> {
        match self {
            Source::Generic | Source::GenericV2 => Some(p.webhook_secret.as_str()),
            Source::Stripe => p.stripe_secret.as_deref(),
            Source::Paddle => p.paddle_secret.as_deref(),
        }
        .filter(|s| !s.is_empty())
    }

    pub fn verify(self, headers: &HeaderMap, body: &[u8], secret: &str) -> Result<(), VerifyError> {
        self.verify_at(headers, body, secret, now_secs())
    }

    pub fn verify_at(
        self,
        headers: &HeaderMap,
        body: &[u8],
        secret: &str,
        now: i64,
    ) -> Result<(), VerifyError> {
        match self {
            // X-Bf-Signature: <hex>, signed `<X-Bf-Timestamp>.<Idempotency-Key>.<body>`. The key
            // is signed because it decides deduplication, and the timestamp bounds replay.
            Source::Generic => {
                let key = idempotency_key(headers).ok_or(VerifyError::Missing)?;
                let ts = header(headers, "x-bf-timestamp").ok_or(VerifyError::Missing)?;
                let sig = header(headers, "x-bf-signature").ok_or(VerifyError::Missing)?;
                let sig = hex_decode(sig).ok_or(VerifyError::Malformed)?;
                let secs: i64 = ts.parse().map_err(|_| VerifyError::Malformed)?;
                let mac = mac(secret, &[ts.as_bytes(), b".", key.as_bytes(), b".", body]);
                mac.verify_slice(&sig).map_err(|_| VerifyError::Mismatch)?;
                if now.abs_diff(secs) > TOLERANCE_SECS as u64 {
                    return Err(VerifyError::Stale);
                }
                Ok(())
            }
            // Bf-Signature only; the v1 headers are not accepted here (no downgrade confusion).
            Source::GenericV2 => {
                let key = idempotency_key(headers).ok_or(VerifyError::Missing)?;
                let h = header(headers, "bf-signature").ok_or(VerifyError::Missing)?;
                timestamped(h, ',', ("t", "v1"), (b".", Some(key)), body, secret, now)
            }
            // Stripe-Signature: t=<ts>,v1=<hex>[,v1=<hex>][,v0=<hex>]; signed `<ts>.<body>`.
            Source::Stripe => {
                let h = header(headers, "stripe-signature").ok_or(VerifyError::Missing)?;
                timestamped(h, ',', ("t", "v1"), (b".", None), body, secret, now)
            }
            // Paddle-Signature: ts=<ts>;h1=<hex>[;h1=<hex>]; signed `<ts>:<body>`.
            Source::Paddle => {
                let h = header(headers, "paddle-signature").ok_or(VerifyError::Missing)?;
                timestamped(h, ';', ("ts", "h1"), (b":", None), body, secret, now)
            }
        }
    }

    /// The dedup key of a verified delivery: Idempotency-Key, Stripe event `id`,
    /// Paddle `event_id`.
    pub fn external_id(self, headers: &HeaderMap, payload: &Value) -> Option<String> {
        let id = match self {
            Source::Generic | Source::GenericV2 => idempotency_key(headers)?,
            Source::Stripe => payload.get("id")?.as_str()?,
            Source::Paddle => payload.get("event_id")?.as_str()?,
        };
        (!id.is_empty() && id.len() <= MAX_EXTERNAL_ID).then(|| id.to_owned())
    }
}

/// Header of `k=v` pairs joined by `sep`: any listed signature may match (secret rotation).
fn timestamped(
    header: &str,
    sep: char,
    (ts_key, sig_key): (&str, &str),
    (glue, key): (&[u8], Option<&str>),
    body: &[u8],
    secret: &str,
    now: i64,
) -> Result<(), VerifyError> {
    let (mut ts, mut sigs) = (None, Vec::new());
    for (k, v) in header.split(sep).filter_map(|p| p.trim().split_once('=')) {
        if k == ts_key {
            ts = Some(v);
        } else if k == sig_key {
            sigs.extend(hex_decode(v));
        }
    }
    let ts = ts.ok_or(VerifyError::Malformed)?;
    let secs: i64 = ts.parse().map_err(|_| VerifyError::Malformed)?;
    if sigs.is_empty() {
        return Err(VerifyError::Malformed);
    }
    // Providers sign `<ts><glue><body>`; Bf-Signature signs `<ts>.<key>.<body>`.
    let mac = match key {
        Some(k) => mac(secret, &[ts.as_bytes(), b".", k.as_bytes(), b".", body]),
        None => mac(secret, &[ts.as_bytes(), glue, body]),
    };
    if !sigs.iter().any(|s| mac.clone().verify_slice(s).is_ok()) {
        return Err(VerifyError::Mismatch);
    }
    if now.abs_diff(secs) > TOLERANCE_SECS as u64 {
        return Err(VerifyError::Stale);
    }
    Ok(())
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn idempotency_key(headers: &HeaderMap) -> Option<&str> {
    header(headers, "idempotency-key").filter(|k| !k.is_empty())
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let nibble = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|p| Some(nibble(p[0])? << 4 | nibble(p[1])?))
        .collect()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures use the documented header layouts; the tags were computed independently
    // (Python hmac) so they pin the signed-payload construction, not this implementation.
    const STRIPE_TS: i64 = 1492774577;
    const STRIPE_BODY: &[u8] =
        br#"{"id":"evt_1NG8Du2eZvKYlo2CUI79vXWy","object":"event","type":"checkout.session.completed"}"#;
    const STRIPE_SIG: &str = "ab1e77e4c00401a0f27e4f73afe4d17121b6d3ab9d0e9b18015d7433cfa77495";
    const PADDLE_TS: i64 = 1671552777;
    const PADDLE_BODY: &[u8] =
        br#"{"event_id":"evt_01hv8wptq8987qeqjtaqfc2g3c","event_type":"transaction.completed"}"#;
    const PADDLE_SIG: &str = "3590e53213c1a8c45bdbfd19fc064f8b8556a52fc2e7645fc3c08d97366a0c81";
    const GENERIC_BODY: &[u8] = br#"{"type":"sale","amount_cents":4900}"#;
    const GENERIC_TS: i64 = 1_700_000_000;
    // HMAC-SHA256("whsec_generic", "1700000000.order-1.<GENERIC_BODY>"), computed with Python.
    const GENERIC_SIG: &str = "a18d4e38b44b600b8a3cfd6badfd79dea2fa77fba632ecffe72db424d6b4045f";

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).parse().unwrap(), v.parse().unwrap()))
            .collect()
    }

    fn stripe(h: &str, now: i64) -> Result<(), VerifyError> {
        let h = headers(&[("stripe-signature", h)]);
        Source::Stripe.verify_at(&h, STRIPE_BODY, "whsec_test_secret", now)
    }

    fn paddle(h: &str, now: i64) -> Result<(), VerifyError> {
        let h = headers(&[("paddle-signature", h)]);
        Source::Paddle.verify_at(&h, PADDLE_BODY, "pdl_ntfset_test_secret", now)
    }

    #[test]
    fn stripe_signature() {
        let ok = format!("t={STRIPE_TS},v1={STRIPE_SIG},v0=deadbeef");
        assert_eq!(stripe(&ok, STRIPE_TS + 10), Ok(()));
        // Rotation: a stale v1 beside a valid one still passes.
        let rotated = format!("t={STRIPE_TS},v1={},v1={STRIPE_SIG}", "00".repeat(32));
        assert_eq!(stripe(&rotated, STRIPE_TS), Ok(()));
        // Tolerance edges: 300s ok, 301s stale, either direction.
        assert_eq!(stripe(&ok, STRIPE_TS + 300), Ok(()));
        assert_eq!(stripe(&ok, STRIPE_TS + 301), Err(VerifyError::Stale));
        assert_eq!(stripe(&ok, STRIPE_TS - 301), Err(VerifyError::Stale));
        // Tampered tag, tampered timestamp, junk.
        let bad = format!("t={STRIPE_TS},v1={}", "0".repeat(64));
        assert_eq!(stripe(&bad, STRIPE_TS), Err(VerifyError::Mismatch));
        let moved = format!("t={},v1={STRIPE_SIG}", STRIPE_TS + 1);
        assert_eq!(stripe(&moved, STRIPE_TS), Err(VerifyError::Mismatch));
        assert_eq!(stripe("v1=zz", STRIPE_TS), Err(VerifyError::Malformed));
        assert_eq!(
            stripe(&format!("t=x,v1={STRIPE_SIG}"), 0),
            Err(VerifyError::Malformed)
        );
        assert_eq!(
            Source::Stripe.verify_at(&HeaderMap::new(), STRIPE_BODY, "s", 0),
            Err(VerifyError::Missing)
        );
        let h = headers(&[("stripe-signature", &ok)]);
        assert_eq!(
            Source::Stripe.verify_at(&h, STRIPE_BODY, "other", STRIPE_TS),
            Err(VerifyError::Mismatch)
        );
    }

    #[test]
    fn paddle_signature() {
        let ok = format!("ts={PADDLE_TS};h1={PADDLE_SIG}");
        assert_eq!(paddle(&ok, PADDLE_TS + 5), Ok(()));
        assert_eq!(paddle(&ok, PADDLE_TS + 301), Err(VerifyError::Stale));
        let bad = format!("ts={PADDLE_TS};h1={}", "1".repeat(64));
        assert_eq!(paddle(&bad, PADDLE_TS), Err(VerifyError::Mismatch));
        assert_eq!(paddle("h1=abcd", PADDLE_TS), Err(VerifyError::Malformed));
        // The Stripe layout does not verify as Paddle.
        let stripe_style = format!("t={PADDLE_TS},v1={PADDLE_SIG}");
        assert_eq!(
            paddle(&stripe_style, PADDLE_TS),
            Err(VerifyError::Malformed)
        );
    }

    #[test]
    fn generic_signature_and_key() {
        let verify = |h: &[(&'static str, &str)], secret, now| {
            Source::Generic.verify_at(&headers(h), GENERIC_BODY, secret, now)
        };
        let ts = GENERIC_TS.to_string();
        let ok = [
            ("x-bf-signature", GENERIC_SIG),
            ("idempotency-key", "order-1"),
            ("x-bf-timestamp", ts.as_str()),
        ];
        assert_eq!(verify(&ok, "whsec_generic", GENERIC_TS + 10), Ok(()));
        assert_eq!(verify(&ok, "nope", GENERIC_TS), Err(VerifyError::Mismatch));
        // Each header is required.
        for skip in 0..3 {
            let partial: Vec<_> = ok
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != skip)
                .map(|(_, h)| *h)
                .collect();
            assert_eq!(
                verify(&partial, "whsec_generic", GENERIC_TS),
                Err(VerifyError::Missing)
            );
        }
        let odd = [
            ("x-bf-signature", "abc"),
            ("idempotency-key", "k"),
            ("x-bf-timestamp", ts.as_str()),
        ];
        assert_eq!(
            verify(&odd, "whsec_generic", GENERIC_TS),
            Err(VerifyError::Malformed)
        );
        let junk_ts = [
            ("x-bf-signature", GENERIC_SIG),
            ("idempotency-key", "order-1"),
            ("x-bf-timestamp", "soon"),
        ];
        assert_eq!(
            verify(&junk_ts, "whsec_generic", GENERIC_TS),
            Err(VerifyError::Malformed)
        );
    }

    #[test]
    fn generic_signature_covers_the_key_and_expires() {
        let ts = GENERIC_TS.to_string();
        let verify = |key: &str, now| {
            let h = [
                ("x-bf-signature", GENERIC_SIG),
                ("idempotency-key", key),
                ("x-bf-timestamp", ts.as_str()),
            ];
            Source::Generic.verify_at(&headers(&h), GENERIC_BODY, "whsec_generic", now)
        };
        // A captured request replayed under a fresh Idempotency-Key no longer verifies.
        assert_eq!(verify("order-2", GENERIC_TS), Err(VerifyError::Mismatch));
        // Tolerance edges, both directions.
        assert_eq!(verify("order-1", GENERIC_TS + 300), Ok(()));
        assert_eq!(verify("order-1", GENERIC_TS + 301), Err(VerifyError::Stale));
        assert_eq!(verify("order-1", GENERIC_TS - 301), Err(VerifyError::Stale));
        // A moved timestamp is a different message.
        let moved = (GENERIC_TS + 1).to_string();
        let h = [
            ("x-bf-signature", GENERIC_SIG),
            ("idempotency-key", "order-1"),
            ("x-bf-timestamp", moved.as_str()),
        ];
        assert_eq!(
            Source::Generic.verify_at(&headers(&h), GENERIC_BODY, "whsec_generic", GENERIC_TS),
            Err(VerifyError::Mismatch)
        );
    }

    // The API's webhook tester signs with bf_core::webhook_sign; every layout must verify here.
    #[test]
    fn tester_signatures_verify() {
        use bf_core::webhook_sign::{Layout, sign};
        let now = 1_700_000_000;
        for (layout, source) in [
            (Layout::Generic, Source::Generic),
            (Layout::Stripe, Source::Stripe),
            (Layout::Paddle, Source::Paddle),
        ] {
            let pairs: Vec<(&'static str, String)> = sign(layout, "sek", "test-1", b"{}", now);
            let h: HeaderMap = pairs
                .iter()
                .map(|(k, v)| ((*k).parse().unwrap(), v.parse().unwrap()))
                .collect();
            assert_eq!(source.verify_at(&h, b"{}", "sek", now), Ok(()));
            assert_eq!(
                source.verify_at(&h, b"{ }", "sek", now),
                Err(VerifyError::Mismatch)
            );
        }
    }

    #[test]
    fn external_ids() {
        let h = headers(&[("idempotency-key", "order-1")]);
        let none = HeaderMap::new();
        let stripe: Value = serde_json::from_slice(STRIPE_BODY).unwrap();
        let paddle: Value = serde_json::from_slice(PADDLE_BODY).unwrap();
        assert_eq!(
            Source::Generic.external_id(&h, &Value::Null).as_deref(),
            Some("order-1")
        );
        assert_eq!(Source::Generic.external_id(&none, &Value::Null), None);
        assert_eq!(
            Source::Stripe.external_id(&none, &stripe).as_deref(),
            Some("evt_1NG8Du2eZvKYlo2CUI79vXWy")
        );
        assert_eq!(
            Source::Paddle.external_id(&none, &paddle).as_deref(),
            Some("evt_01hv8wptq8987qeqjtaqfc2g3c")
        );
        assert_eq!(Source::Paddle.external_id(&none, &stripe), None);
        let long = headers(&[("idempotency-key", &"k".repeat(256))]);
        assert_eq!(Source::Generic.external_id(&long, &Value::Null), None);
    }
}
