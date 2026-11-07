//! Separate binary: the SSRF guard is process-global and must stay on here.

use std::sync::Arc;

use api::auth::{Principal, issue_access};
use api::{AppState, app};
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{Method, Request, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn notify_channel_url_goes_through_ssrf_guard(pool: PgPool) {
    let ring = KeyRing::from_pairs("t1", &[("t1", [5u8; 32])]).unwrap();
    let router = app(AppState {
        pool: pool.clone(),
        jwt_keys: api::auth::JwtKeys::single("s").into(),
        version_cache: Default::default(),
        tracker_url: Default::default(),
        vault: Arc::new(Vault::new(ring)),
        secure_cookies: true,
        limiter: Default::default(),
    });
    let ws = Uuid::now_v7();
    create_workspace(&pool, ws, "w", "w").await.unwrap();
    let u = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: "a@x.io".into(),
        password_hash: "unused".into(),
        role: UserRole::Admin,
    };
    create_user(&pool, &u).await.unwrap();
    let cookie = format!("bf_at={}", issue_access("s", &Principal::from(&u)).unwrap());
    for (k, url) in [
        ("discord_url", "http://10.0.0.5/hook"),
        ("telegram_url", "http://169.254.169.254/latest"),
        ("discord_url", "ftp://example.com/x"),
        ("discord_url", "not a url"),
    ] {
        let req = Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/settings/notify-channels")
            .header(HOST, "bf.test")
            .header(ORIGIN, "https://bf.test")
            .header("x-csrf-token", "t")
            .header(COOKIE, format!("{cookie}; bf_csrf=t"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ k: url }).to_string()))
            .unwrap();
        let res = router.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{url}");
    }
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspaces WHERE id = $1 AND (notify_discord_webhook IS NOT NULL OR notify_telegram_webhook IS NOT NULL)",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
}
