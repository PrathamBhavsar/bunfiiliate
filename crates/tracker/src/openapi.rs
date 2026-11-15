//! OpenAPI 3.1 document for the tracker: `/v2/*`, `/t.js`, `/r/{code}`, health.
//! Served at `GET /v2/openapi.json`; snapshot in `docs/openapi/tracker-v2.json`
//! (`tracker openapi` prints it, `make tracker-openapi-check` diffs it).
#![allow(dead_code)]

use axum::response::{IntoResponse, Response};
use utoipa::{OpenApi, ToSchema};

/// Every `/v2` route the router registers: `(method, path)`. The contract test pins this to
/// the spec and to the route literals in `lib.rs`.
pub const V2_ROUTES: &[(&str, &str)] = &[
    ("get", "/v2/openapi.json"),
    ("post", "/v2/c"),
    ("post", "/v2/events/{key}"),
    ("post", "/v2/webhooks/paddle/{key}"),
    ("post", "/v2/webhooks/stripe/{key}"),
];

/// Error body (all `/v2` routes), `error.schema.json`.
#[derive(ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(ToSchema)]
pub struct ErrorDetail {
    /// `^[a-z][a-z0-9_]*$`
    pub code: String,
    pub message: String,
    pub request_id: Option<String>,
}

/// `bf.event.v2`, `event.schema.json`.
#[derive(ToSchema)]
pub struct EventV2 {
    /// Const `bf.event.v2`.
    pub schema: String,
    /// `sale|signup|lead|refund|chargeback|custom:<name>`.
    #[schema(rename = "type")]
    pub kind: String,
    #[schema(min_length = 1, max_length = 128)]
    pub external_id: String,
    /// RFC 3339 with offset.
    pub occurred_at: String,
    pub click_id: Option<String>,
    pub code: Option<String>,
    pub customer: Option<Customer>,
    /// Integer cents, `>= 0`; required for sale, refund, chargeback.
    #[schema(minimum = 0)]
    pub amount_cents: Option<u64>,
    /// Three uppercase letters.
    pub currency: Option<String>,
    /// Required for refund and chargeback.
    pub ref_external_id: Option<String>,
    /// Flat map (string, number, bool), at most 20 keys.
    pub meta: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(ToSchema)]
pub struct Customer {
    pub external_id: Option<String>,
    /// 64 lowercase hex.
    pub email_sha256: Option<String>,
}

/// `click-beacon.schema.json`.
#[derive(ToSchema)]
pub struct ClickBeacon {
    /// Const 2.
    pub v: u8,
    pub pk: Option<String>,
    pub code: Option<String>,
    pub click: Option<String>,
    /// 8..64 of `[A-Za-z0-9_-]`.
    pub vid: String,
    /// http(s), at most 2048.
    pub url: String,
    #[schema(rename = "ref")]
    pub referrer: Option<String>,
    pub ts: Option<u64>,
}

#[derive(ToSchema)]
pub struct Accepted {
    /// `accepted` or `duplicate`.
    pub status: String,
    pub id: uuid::Uuid,
}

#[utoipa::path(post, path = "/v2/events/{key}", tag = "v2",
    params(
        ("key" = String, Path, description = "Program public key"),
        ("Idempotency-Key" = String, Header, description = "[A-Za-z0-9_-]{1,64}"),
        ("Bf-Signature" = String, Header, description = "t=<unix>,v1=<hex hmac-sha256 of `<t>.<key>.<body>`>"),
    ),
    request_body = EventV2,
    responses(
        (status = 202, description = "Accepted", body = Accepted),
        (status = 200, description = "Duplicate of an earlier event", body = Accepted),
        (status = 400, description = "Invalid request", body = ErrorBody),
        (status = 401, description = "Bad signature", body = ErrorBody),
        (status = 404, description = "Unknown program", body = ErrorBody),
        (status = 409, description = "Idempotency key reused with a different body", body = ErrorBody),
        (status = 413, description = "Body too large", body = ErrorBody),
        (status = 503, description = "Shedding load, retry", body = ErrorBody),
    ))]
fn post_event() {}

#[utoipa::path(post, path = "/v2/webhooks/stripe/{key}", tag = "v2",
    params(
        ("key" = String, Path, description = "Program public key"),
        ("Stripe-Signature" = String, Header, description = "Stripe signature"),
    ),
    request_body(content = String, content_type = "application/json"),
    responses(
        (status = 200, description = "Accepted or duplicate (`status` says which)", body = Accepted),
        (status = 400, description = "Invalid request", body = ErrorBody),
        (status = 401, description = "Bad signature", body = ErrorBody),
        (status = 404, description = "Unknown program", body = ErrorBody),
        (status = 409, description = "Conflict", body = ErrorBody),
        (status = 413, description = "Body too large", body = ErrorBody),
        (status = 503, description = "Shedding load, retry", body = ErrorBody),
    ))]
fn post_stripe() {}

#[utoipa::path(post, path = "/v2/webhooks/paddle/{key}", tag = "v2",
    params(
        ("key" = String, Path, description = "Program public key"),
        ("Paddle-Signature" = String, Header, description = "Paddle signature"),
    ),
    request_body(content = String, content_type = "application/json"),
    responses(
        (status = 200, description = "Accepted or duplicate (`status` says which)", body = Accepted),
        (status = 400, description = "Invalid request", body = ErrorBody),
        (status = 401, description = "Bad signature", body = ErrorBody),
        (status = 404, description = "Unknown program", body = ErrorBody),
        (status = 409, description = "Conflict", body = ErrorBody),
        (status = 413, description = "Body too large", body = ErrorBody),
        (status = 503, description = "Shedding load, retry", body = ErrorBody),
    ))]
fn post_paddle() {}

#[utoipa::path(post, path = "/v2/c", tag = "v2",
    request_body = ClickBeacon,
    responses(
        (status = 204, description = "Recorded"),
        (status = 400, description = "Invalid request", body = ErrorBody),
        (status = 404, description = "Unknown code", body = ErrorBody),
        (status = 413, description = "Body too large", body = ErrorBody),
        (status = 503, description = "Shedding load, retry", body = ErrorBody),
    ))]
fn post_beacon() {}

#[utoipa::path(get, path = "/v2/openapi.json", tag = "v2",
    responses((status = 200, description = "This document", content_type = "application/json")))]
fn get_openapi() {}

#[utoipa::path(get, path = "/t.js", tag = "assets",
    responses(
        (status = 200, description = "Tracking script", content_type = "application/javascript"),
        (status = 304, description = "Not modified"),
    ))]
fn get_tracker_js() {}

#[utoipa::path(get, path = "/r/{code}", tag = "redirect",
    params(("code" = String, Path, description = "Referral code")),
    responses(
        (status = 302, description = "Redirect to the landing URL"),
        (status = 404, description = "Unknown code"),
    ))]
fn get_redirect() {}

#[utoipa::path(get, path = "/healthz", tag = "health",
    responses((status = 200, description = "Alive")))]
fn get_healthz() {}

#[utoipa::path(get, path = "/readyz", tag = "health",
    responses(
        (status = 200, description = "Ready"),
        (status = 503, description = "Database unreachable"),
    ))]
fn get_readyz() {}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "bunfiiliate tracker",
        version = "2",
        description = "Tracker contract v2"
    ),
    paths(
        post_event,
        post_stripe,
        post_paddle,
        post_beacon,
        get_openapi,
        get_tracker_js,
        get_redirect,
        get_healthz,
        get_readyz
    ),
    components(schemas(ErrorBody, ErrorDetail, EventV2, Customer, ClickBeacon, Accepted))
)]
pub struct Doc;

/// Pretty JSON of the document (trailing newline excluded).
pub fn json() -> String {
    Doc::openapi().to_pretty_json().expect("openapi serializes")
}

pub async fn serve() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json(),
    )
        .into_response()
}
