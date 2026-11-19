//! Signup is off unless `SIGNUP_ENABLED` is set. This binary never sets it.

use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{Request, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

fn app(pool: PgPool) -> Router {
    let ring = KeyRing::from_pairs("t1", &[("t1", [7u8; 32])]).unwrap();
    let state = AppState {
        pool,
        jwt_keys: api::auth::JwtKeys::single("test-secret").into(),
        version_cache: Default::default(),
        tracker_url: Default::default(),
        vault: std::sync::Arc::new(Vault::new(ring)),
        secure_cookies: true,
        limiter: Default::default(),
    };
    build(routes::routes(), state)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_disabled_by_default(pool: PgPool) {
    let app = app(pool.clone());
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/signup")
        .header(HOST, "bf.test")
        .header(ORIGIN, "https://bf.test")
        .header(COOKIE, "bf_csrf=t")
        .header("x-csrf-token", "t")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"workspace_name": "Acme", "admin_email": "a@x.io", "admin_password": "long enough pw"}).to_string(),
        ))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM workspaces")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn config_reports_signup_disabled(pool: PgPool) {
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/config")
        .header(HOST, "bf.test")
        .body(Body::empty())
        .unwrap();
    let res = app(pool).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body, json!({ "signup_enabled": false }));
}
