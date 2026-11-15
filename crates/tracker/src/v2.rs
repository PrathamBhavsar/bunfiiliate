//! Tracker contract v2 (`docs/tracker-contract-v2.md`): strict inbound DTOs and validators.
//! `deny_unknown_fields` applies here only; v1 bodies stay lenient. Everything runs before
//! the spool (anti-pattern 16), so a body Postgres would reject never reaches the writer.

use axum::http::{HeaderMap, HeaderValue, StatusCode, header::RETRY_AFTER};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const MAX_URL: usize = 2048;
const MAX_ID: usize = 64;
const MAX_EXTERNAL_ID: usize = 128;
const MAX_META_KEYS: usize = 20;
const MAX_META_KEY: usize = 64;
const MAX_META_STR: usize = 512;
const MAX_META_BYTES: usize = 2048;

/// A `/v2` error: `{"error":{"code","message","request_id"}}`.
#[derive(Debug, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    /// Map a status from the shared ingest path (503 = shed or spool trouble).
    pub fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::SERVICE_UNAVAILABLE => {
                Self::new(status, "overloaded", "server is shedding load, retry")
            }
            _ => Self::new(status, "internal_error", "internal error"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({"error": {
            "code": self.code,
            "message": self.message,
            "request_id": Uuid::now_v7().to_string(),
        }});
        let mut res = (self.status, axum::Json(body)).into_response();
        if self.status == StatusCode::SERVICE_UNAVAILABLE {
            res.headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static("1"));
        }
        res
    }
}

/// Rewrite framework-made bodies (413 from the body limit is plain text) into the error schema.
pub async fn json_errors(res: Response) -> Response {
    let is_json = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if res.status() == StatusCode::PAYLOAD_TOO_LARGE && !is_json {
        return ApiError::new(res.status(), "payload_too_large", "body too large").into_response();
    }
    res
}

/// `[A-Za-z0-9_-]{min..=max}`.
fn token(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `Idempotency-Key`: `[A-Za-z0-9_-]{1,64}`.
pub fn valid_idempotency_key(k: &str) -> bool {
    token(k, 1, MAX_ID)
}

/// Program public key in the path.
pub fn valid_program_key(k: &str) -> bool {
    token(k, 1, MAX_ID)
}

/// jsonb cannot hold a NUL escape. Checked on the raw body so it covers every string.
pub fn has_nul(body: &[u8]) -> bool {
    body.contains(&0) || body.windows(6).any(|w| w == b"\\u0000")
}

fn plain(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}

// ---------------------------------------------------------------- click beacon

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Beacon {
    pub v: u8,
    #[serde(default)]
    pub pk: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub click: Option<String>,
    pub vid: String,
    pub url: String,
    #[serde(default, rename = "ref")]
    pub referrer: Option<String>,
    #[serde(default)]
    pub ts: Option<u64>,
}

pub fn parse_beacon(body: &[u8]) -> Result<Beacon, ApiError> {
    if has_nul(body) {
        return Err(ApiError::invalid("NUL character in body"));
    }
    let b: Beacon = serde_json::from_slice(body).map_err(|e| ApiError::invalid(e.to_string()))?;
    if b.v != 2 {
        return Err(ApiError::invalid("v must be 2"));
    }
    for (name, val) in [("pk", &b.pk), ("code", &b.code), ("click", &b.click)] {
        if val.as_deref().is_some_and(|s| !token(s, 1, MAX_ID)) {
            return Err(ApiError::invalid(format!("bad {name}")));
        }
    }
    if b.code.is_none() && b.click.is_none() {
        return Err(ApiError::invalid("code or click required"));
    }
    if !token(&b.vid, 8, MAX_ID) {
        return Err(ApiError::invalid("vid must be 8..64 of [A-Za-z0-9_-]"));
    }
    let http = b.url.starts_with("http://") || b.url.starts_with("https://");
    if !http || b.url.len() > MAX_URL {
        return Err(ApiError::invalid("url must be http(s), <= 2048"));
    }
    if b.referrer.as_deref().is_some_and(|r| !plain(r, MAX_URL)) {
        return Err(ApiError::invalid("bad ref"));
    }
    Ok(b)
}

// ----------------------------------------------------------------------- event

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Customer {
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub email_sha256: Option<String>,
}

/// `bf.event.v2`. `amount_cents` is a `u64`, so serde_json rejects `49.0`, `4.9e3` and `-1`
/// on the number token itself, not after a float round trip.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventV2 {
    pub schema: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub external_id: String,
    pub occurred_at: String,
    #[serde(default)]
    pub click_id: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub customer: Option<Customer>,
    #[serde(default)]
    pub amount_cents: Option<u64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub ref_external_id: Option<String>,
    #[serde(default)]
    pub meta: Option<Map<String, Value>>,
}

fn valid_type(t: &str) -> bool {
    match t.strip_prefix("custom:") {
        Some(name) => {
            (1..=32).contains(&name.len())
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        }
        None => matches!(t, "sale" | "signup" | "lead" | "refund" | "chargeback"),
    }
}

/// RFC 3339 `date-time` with a mandatory offset.
fn valid_date_time(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| {
        b.get(r.clone())
            .filter(|d| d.iter().all(u8::is_ascii_digit))
            .and_then(|d| std::str::from_utf8(d).ok()?.parse::<u32>().ok())
    };
    let sep = |i: usize, c: &[u8]| b.get(i).is_some_and(|x| c.contains(x));
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) = (
        digits(0..4),
        digits(5..7),
        digits(8..10),
        digits(11..13),
        digits(14..16),
        digits(17..19),
    ) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if !(sep(4, b"-") && sep(7, b"-") && sep(10, b"Tt") && sep(13, b":") && sep(16, b":"))
        || !(1..=days).contains(&d)
        || h > 23
        || mi > 59
        || sec > 60
    {
        return false;
    }
    let mut i = 19;
    if sep(i, b".") {
        let frac = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if frac == 0 {
            return false;
        }
        i += 1 + frac;
    }
    match b.get(i) {
        Some(b'Z' | b'z') => i + 1 == b.len(),
        Some(b'+' | b'-') => {
            i + 6 == b.len()
                && sep(i + 3, b":")
                && digits(i + 1..i + 3).is_some_and(|x| x < 24)
                && digits(i + 4..i + 6).is_some_and(|x| x < 60)
        }
        _ => false,
    }
}

fn valid_meta(meta: &Map<String, Value>) -> bool {
    meta.len() <= MAX_META_KEYS
        && meta.iter().all(|(k, v)| {
            plain(k, MAX_META_KEY)
                && match v {
                    Value::String(s) => plain(s, MAX_META_STR),
                    Value::Number(_) | Value::Bool(_) => true,
                    _ => false,
                }
        })
        && serde_json::to_vec(meta).is_ok_and(|b| b.len() <= MAX_META_BYTES)
}

/// A validated event and its JSON form for `inbound_events.payload`.
pub struct Parsed {
    pub event: EventV2,
    pub payload: Value,
}

pub fn parse_event(body: &[u8]) -> Result<Parsed, ApiError> {
    if has_nul(body) {
        return Err(ApiError::invalid("NUL character in body"));
    }
    let event: EventV2 =
        serde_json::from_slice(body).map_err(|e| ApiError::invalid(e.to_string()))?;
    check_event(&event).map_err(ApiError::invalid)?;
    let payload = serde_json::from_slice(body).map_err(|e| ApiError::invalid(e.to_string()))?;
    Ok(Parsed { event, payload })
}

fn check_event(e: &EventV2) -> Result<(), &'static str> {
    if e.schema != "bf.event.v2" {
        return Err("schema must be bf.event.v2");
    }
    if !valid_type(&e.kind) {
        return Err("bad type");
    }
    if e.external_id.is_empty() || !plain(&e.external_id, MAX_EXTERNAL_ID) {
        return Err("external_id must be 1..128");
    }
    if !valid_date_time(&e.occurred_at) {
        return Err("occurred_at must be RFC 3339");
    }
    for v in [&e.click_id, &e.code] {
        if v.as_deref().is_some_and(|s| !token(s, 1, MAX_ID)) {
            return Err("bad click_id or code");
        }
    }
    if let Some(c) = &e.customer {
        if c.external_id
            .as_deref()
            .is_some_and(|s| s.is_empty() || !plain(s, MAX_EXTERNAL_ID))
        {
            return Err("bad customer.external_id");
        }
        let hex = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if c.email_sha256.as_deref().is_some_and(|s| !hex(s)) {
            return Err("customer.email_sha256 must be 64 lowercase hex");
        }
    }
    if e.amount_cents.is_some_and(|a| a > i64::MAX as u64) {
        return Err("amount_cents too large");
    }
    if e.currency
        .as_deref()
        .is_some_and(|c| c.len() != 3 || !c.bytes().all(|b| b.is_ascii_uppercase()))
    {
        return Err("currency must be 3 uppercase letters");
    }
    if e.ref_external_id
        .as_deref()
        .is_some_and(|s| s.is_empty() || !plain(s, MAX_EXTERNAL_ID))
    {
        return Err("bad ref_external_id");
    }
    if matches!(e.kind.as_str(), "sale" | "refund" | "chargeback")
        && (e.amount_cents.is_none() || e.currency.is_none())
    {
        return Err("amount_cents and currency required");
    }
    if matches!(e.kind.as_str(), "refund" | "chargeback") && e.ref_external_id.is_none() {
        return Err("ref_external_id required");
    }
    if e.meta.as_ref().is_some_and(|m| !valid_meta(m)) {
        return Err("bad meta");
    }
    Ok(())
}

/// A header value that must be present and ASCII.
pub fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn examples() -> Vec<(String, Vec<u8>)> {
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/tracker/v2/examples");
        let mut out: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .map(|p| {
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    fs::read(&p).unwrap(),
                )
            })
            .collect();
        out.sort();
        out
    }

    /// The Rust validators agree with every schema example (valid passes, invalid fails).
    #[test]
    fn v2_examples_agree_with_schemas() {
        let mut seen = 0;
        for (name, body) in examples() {
            let ok = if name.starts_with("event.") {
                parse_event(&body).is_ok()
            } else if name.starts_with("click-beacon.") {
                parse_beacon(&body).is_ok()
            } else {
                continue;
            };
            seen += 1;
            assert_eq!(ok, name.ends_with(".valid.json"), "{name}");
        }
        assert!(seen > 30, "only {seen} examples checked");
    }

    #[test]
    fn v2_event_amount_must_be_an_integer_token() {
        let sale = |amt: &str| {
            format!(
                r#"{{"schema":"bf.event.v2","type":"sale","external_id":"o1","occurred_at":"2026-01-01T12:00:00Z","amount_cents":{amt},"currency":"USD"}}"#
            )
        };
        assert!(parse_event(sale("4900").as_bytes()).is_ok());
        for bad in ["49.0", "4.9e3", "4900.5", "-1", "1e2", "\"4900\"", "null"] {
            assert!(parse_event(sale(bad).as_bytes()).is_err(), "{bad}");
        }
        assert!(parse_event(sale("18446744073709551615").as_bytes()).is_err());
    }

    #[test]
    fn v2_idempotency_key_charset() {
        assert!(valid_idempotency_key("ord_1-A"));
        assert!(valid_idempotency_key(&"a".repeat(64)));
        for bad in ["", "a b", "a.b", "a/b", "é", &"a".repeat(65)] {
            assert!(!valid_idempotency_key(bad), "{bad:?}");
        }
    }

    #[test]
    fn v2_date_time() {
        for ok in [
            "2026-01-01T12:00:00Z",
            "2026-02-28T23:59:59.123+05:30",
            "2024-02-29T00:00:00-08:00",
        ] {
            assert!(valid_date_time(ok), "{ok}");
        }
        for bad in [
            "2026-01-01",
            "2026-02-30T00:00:00Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T12:00:00",
            "2026-01-01T12:00:00.Z",
            "2026-01-01 12:00:00Z",
            "2026-01-01T12:00:00+0530",
        ] {
            assert!(!valid_date_time(bad), "{bad}");
        }
    }

    /// Arbitrary bytes never panic; mutated valid bodies either parse or error.
    #[test]
    fn v2_event_parser_total() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let base = br#"{"schema":"bf.event.v2","type":"sale","external_id":"o1","occurred_at":"2026-01-01T12:00:00Z","amount_cents":4900,"currency":"USD","meta":{"a":1}}"#;
        for _ in 0..5000 {
            let mut v = base.to_vec();
            for _ in 0..(next() % 4) {
                let i = (next() as usize) % v.len();
                v[i] = next() as u8;
            }
            let _ = parse_event(&v);
            let n = (next() % 200) as usize;
            let junk: Vec<u8> = (0..n).map(|_| next() as u8).collect();
            let _ = parse_event(&junk);
            let _ = parse_beacon(&junk);
        }
    }

    #[test]
    fn v2_meta_limits() {
        let ev = |meta: &str| {
            format!(
                r#"{{"schema":"bf.event.v2","type":"signup","external_id":"o1","occurred_at":"2026-01-01T12:00:00Z","meta":{meta}}}"#
            )
        };
        assert!(parse_event(ev(r#"{"a":"x","b":1,"c":true}"#).as_bytes()).is_ok());
        assert!(parse_event(ev(r#"{"a":{"n":1}}"#).as_bytes()).is_err());
        assert!(parse_event(ev(r#"{"a":[1]}"#).as_bytes()).is_err());
        assert!(parse_event(ev(r#"{"a":null}"#).as_bytes()).is_err());
        let many: Vec<String> = (0..21).map(|i| format!(r#""k{i}":1"#)).collect();
        assert!(parse_event(ev(&format!("{{{}}}", many.join(","))).as_bytes()).is_err());
        let big = format!(
            r#"{{"a":"{}","b":"{}","c":"{}","d":"{}","e":"{}"}}"#,
            "x".repeat(500),
            "x".repeat(500),
            "x".repeat(500),
            "x".repeat(500),
            "x".repeat(500)
        );
        assert!(parse_event(ev(&big).as_bytes()).is_err());
    }
}
