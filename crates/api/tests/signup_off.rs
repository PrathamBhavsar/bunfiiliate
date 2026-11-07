//! Signup is off unless `SIGNUP_ENABLED` is set. This binary never sets it.

use api::{AppState, build, routes};
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{Request, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_disabled_by_default(pool: PgPool) {
    let ring = KeyRing::from_pairs("t1", &[("t1", [7u8; 32])]).unwrap();
    let state = AppState {
        pool: pool.clone(),
        jwt_keys: api::auth::JwtKeys::single("test-secret").into(),
        version_cache: Default::default(),
        tracker_url: Default::default(),
        vault: std::sync::Arc::new(Vault::new(ring)),
        secure_cookies: true,
        limiter: Default::default(),
    };
    let app = build(routes::routes(), state);
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
