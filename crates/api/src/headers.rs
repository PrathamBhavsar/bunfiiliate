//! Security headers on every API response.

use std::sync::LazyLock;

use axum::extract::Request;
use axum::http::header::{
    CACHE_CONTROL, HeaderName, REFERRER_POLICY, STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// `BF_HSTS=1` turns on `Strict-Transport-Security` (prod compose sets it; plain-http dev must not).
static HSTS: LazyLock<bool> = LazyLock::new(|| std::env::var("BF_HSTS").is_ok_and(|v| v == "1"));

fn apply(h: &mut HeaderMap, path: &str, hsts: bool) {
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    h.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    h.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    if hsts {
        h.insert(
            STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=63072000; includeSubDomains"),
        );
    }
    if path.starts_with("/api/v1/auth/") {
        h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
}

/// Outermost middleware: success and error responses alike carry the headers.
pub async fn security_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    apply(res.headers_mut(), &path, *HSTS);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_hsts_only_when_enabled() {
        let mut h = HeaderMap::new();
        apply(&mut h, "/api/v1/x", false);
        assert!(!h.contains_key(STRICT_TRANSPORT_SECURITY));
        apply(&mut h, "/api/v1/x", true);
        assert!(h.contains_key(STRICT_TRANSPORT_SECURITY));
        assert!(!h.contains_key(CACHE_CONTROL));
    }
}
