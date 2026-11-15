//! A13: v1 machine routes carry Deprecation/Sunset/Link; v2, /t.js and /r do not; v1 behaviour
//! is unchanged; `/t.js` serves the v2 script.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, Response, StatusCode};
use bf_spool::SpoolConfig;
use sqlx::PgPool;
use tower::ServiceExt;
use tracker::geo::Geo;
use tracker::{router, start};
use uuid::Uuid;

async fn app(pool: PgPool) -> (Router, PathBuf) {
    let dir = std::env::temp_dir().join(format!("bf-dep-test-{}", Uuid::now_v7()));
    let t = start(
        pool,
        SpoolConfig::new(&dir),
        "test-salt",
        Geo::disabled(),
        Arc::new(bf_core::secrets::Vault::plaintext()),
    )
    .await
    .unwrap();
    (router(t.state.clone()), dir)
}

fn post(path: &str, body: &str) -> Request<Body> {
    Request::post(path)
        .header("content-type", "application/json")
        .header("idempotency-key", "k1")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn deprecated(res: &Response<Body>) -> bool {
    res.headers().contains_key("deprecation")
        || res.headers().contains_key("sunset")
        || res.headers().contains_key("link")
}

const V1: [&str; 4] = [
    "/c",
    "/v1/events/pk",
    "/v1/webhooks/stripe/pk",
    "/v1/webhooks/paddle/pk",
];
const V2: [&str; 4] = [
    "/v2/c",
    "/v2/events/pk",
    "/v2/webhooks/stripe/pk",
    "/v2/webhooks/paddle/pk",
];

#[sqlx::test(migrations = "../db/migrations")]
async fn v1_routes_carry_deprecation_headers(pool: PgPool) {
    let (app, dir) = app(pool).await;
    for path in V1 {
        // Success or failure, the headers ride along.
        let res = app.clone().oneshot(post(path, "{}")).await.unwrap();
        let h = res.headers();
        assert_eq!(h["deprecation"], "true", "{path}");
        let sunset = h["sunset"].to_str().unwrap();
        assert!(
            sunset.ends_with(" GMT"),
            "{path}: RFC 7231 date, got {sunset}"
        );
        let link = h["link"].to_str().unwrap();
        assert!(
            link.contains("tracker-migration-v1-to-v2.md") && link.contains(r#"rel="deprecation""#),
            "{path}: {link}"
        );
    }
    // The CORS preflight for /c is a v1 answer too, but must still succeed.
    let pre = Request::options("/c")
        .header("origin", "https://shop.example")
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(pre).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_routes_have_no_deprecation_headers(pool: PgPool) {
    let (app, dir) = app(pool).await;
    for path in V2 {
        let res = app.clone().oneshot(post(path, "{}")).await.unwrap();
        assert!(!deprecated(&res), "{path}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn t_js_and_redirect_have_no_deprecation_headers(pool: PgPool) {
    let (app, dir) = app(pool).await;
    for path in ["/t.js", "/r/nope", "/healthz"] {
        let res = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(!deprecated(&res), "{path}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn t_js_serves_v2_script_and_old_pinned_copy_still_posts_to_c(pool: PgPool) {
    let (app, dir) = app(pool).await;
    let res = app
        .oneshot(Request::get("/t.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], include_bytes!("../js/tracker.v2.min.js"));
    let js = std::str::from_utf8(&body).unwrap();
    assert!(js.contains("/v2/c"));
    // The v1 script is kept in the crate: pinned copies post to /c, which stays routed.
    assert!(include_str!("../js/tracker.min.js").contains("/c"));
    std::fs::remove_dir_all(&dir).ok();
}

/// Golden replay of v1 answers: status and body shapes the old tests rely on.
#[sqlx::test(migrations = "../db/migrations")]
async fn v1_behaviour_unchanged(pool: PgPool) {
    let (app, dir) = app(pool).await;
    let cases: [(&str, &str, &str, StatusCode); 4] = [
        (
            "/c",
            "text/plain;charset=UTF-8",
            "not json",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/v1/events/pk",
            "application/json",
            "{}",
            StatusCode::NOT_FOUND,
        ),
        (
            "/v1/webhooks/stripe/pk",
            "application/json",
            "{}",
            StatusCode::NOT_FOUND,
        ),
        (
            "/v1/webhooks/paddle/pk",
            "application/json",
            "{}",
            StatusCode::NOT_FOUND,
        ),
    ];
    for (path, ct, body, want) in cases {
        let req = Request::post(path)
            .header("content-type", ct)
            .body(Body::from(body))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), want, "{path}");
        // v1 errors stay plain (no v2 JSON error envelope).
        assert!(
            res.headers()
                .get("content-type")
                .is_none_or(|v| !v.to_str().unwrap().contains("json")),
            "{path}"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v1_counter_increments(pool: PgPool) {
    let (app, dir) = app(pool).await;
    let before = tracker::metrics::render();
    let n = |s: &str, route: &str| -> u64 {
        let key = format!("bf_tracker_v1_requests_total{{route=\"{route}\"}} ");
        s.lines()
            .find_map(|l| l.strip_prefix(&key))
            .unwrap_or_else(|| panic!("no {key} in metrics"))
            .parse()
            .unwrap()
    };
    for route in ["/c", "/v1/events/{key}"] {
        let path = route.replace("{key}", "pk");
        let b = n(&before, route);
        app.clone().oneshot(post(&path, "{}")).await.unwrap();
        // Other tests share the process counter: only a lower bound is stable.
        assert!(n(&tracker::metrics::render(), route) > b, "{route}");
    }
    std::fs::remove_dir_all(&dir).ok();
}
