//! v1 machine endpoints are deprecated, not removed: they keep their behaviour and gain
//! `Deprecation`, `Sunset` and `Link` headers. `/t.js`, `/r/{code}` and `/v2/*` are untouched.

use std::sync::LazyLock;

use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

use crate::metrics;

/// Default sunset (RFC 7231 IMF-fixdate); override with `BF_V1_SUNSET`.
pub const DEFAULT_SUNSET: &str = "Sun, 01 Nov 2027 00:00:00 GMT";
const LINK: &str = r#"</docs/tracker-migration-v1-to-v2.md>; rel="deprecation""#;

static SUNSET: LazyLock<HeaderValue> =
    LazyLock::new(|| sunset_value(std::env::var("BF_V1_SUNSET").ok().as_deref()));

/// A usable header value, else the default (a bad env value must not break ingest).
fn sunset_value(raw: Option<&str>) -> HeaderValue {
    raw.map(str::trim)
        .filter(|s| s.ends_with(" GMT"))
        .and_then(|s| HeaderValue::from_str(s).ok())
        .unwrap_or_else(|| HeaderValue::from_static(DEFAULT_SUNSET))
}

/// Route layer for v1 routes: count the request and mark the response.
pub async fn v1(path: MatchedPath, req: Request, next: Next) -> Response {
    metrics::v1_request(path.as_str());
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        HeaderName::from_static("deprecation"),
        HeaderValue::from_static("true"),
    );
    h.insert(HeaderName::from_static("sunset"), SUNSET.clone());
    h.append(axum::http::header::LINK, HeaderValue::from_static(LINK));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_sunset_falls_back() {
        assert_eq!(sunset_value(None), DEFAULT_SUNSET);
        assert_eq!(sunset_value(Some("tomorrow")), DEFAULT_SUNSET);
        assert_eq!(
            sunset_value(Some("Fri, 01 Jan 2027 00:00:00 GMT")),
            "Fri, 01 Jan 2027 00:00:00 GMT"
        );
    }
}
