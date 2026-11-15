//! Spool disk full: per-route behaviour (REVISIONS C11).
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bf_spool::SpoolConfig;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;
use tracker::geo::Geo;
use tracker::{router, start};
use uuid::Uuid;

const GENERIC: &str = "generic-secret";

fn dir() -> PathBuf {
    std::env::temp_dir().join(format!("bf-shed-test-{}", Uuid::now_v7()))
}

async fn seed(pool: &PgPool) {
    let (ws, user, aff, prog) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let q = |sql: &'static str| sqlx::query(sql);
    q("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', 'w')")
        .bind(ws)
        .execute(pool)
        .await
        .unwrap();
    q("INSERT INTO users (id, workspace_id, email, password_hash, role) VALUES ($1, $2, 'a@x.io', 'h', 'affiliate')")
        .bind(user).bind(ws).execute(pool).await.unwrap();
    q("INSERT INTO affiliates (id, workspace_id, user_id) VALUES ($1, $2, $3)")
        .bind(aff)
        .bind(ws)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    q("INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) VALUES ($1, $2, 'p', 'USD', 'pk', $3)")
        .bind(prog).bind(ws).bind(GENERIC).execute(pool).await.unwrap();
    q("INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) VALUES ('abc', $1, $2, 'https://example.com/p')")
        .bind(aff).bind(prog).execute(pool).await.unwrap();
}

fn beacon() -> Request<Body> {
    Request::post("/c")
        .header("content-type", "text/plain")
        .header("user-agent", "Mozilla/5.0 Chrome/120.0")
        .body(Body::from(
            r#"{"code":"abc","vid":"v","url":"https://example.com/a"}"#,
        ))
        .unwrap()
}

fn event() -> Request<Body> {
    let (ts, key, body) = (
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string(),
        "k1",
        r#"{"type":"sale","amount_cents":100}"#,
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(GENERIC.as_bytes()).unwrap();
    for p in [ts.as_bytes(), b".", key.as_bytes(), b".", body.as_bytes()] {
        mac.update(p);
    }
    let sig: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Request::post("/v1/events/pk")
        .header("content-type", "application/json")
        .header("x-bf-signature", sig)
        .header("idempotency-key", key)
        .header("x-bf-timestamp", ts)
        .body(Body::from(body))
        .unwrap()
}

async fn up(pool: &PgPool) -> (tracker::Tracker, axum::Router, PathBuf) {
    seed(pool).await;
    let d = dir();
    let t = start(
        pool.clone(),
        SpoolConfig::new(&d),
        "test-salt",
        Geo::disabled(),
        Arc::new(bf_core::secrets::Vault::plaintext()),
    )
    .await
    .unwrap();
    let app = router(t.state.clone());
    (t, app, d)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn shedding_returns_503_with_retry_after(pool: PgPool) {
    let (t, app, d) = up(&pool).await;
    t.state.spool.set_shedding(true);
    for req in [beacon(), event()] {
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(res.headers()["retry-after"], "5");
    }
    std::fs::remove_dir_all(d).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn redirect_still_302_while_shedding_but_records_no_click(pool: PgPool) {
    let (t, app, d) = up(&pool).await;
    t.state.spool.set_shedding(true);
    let res = app
        .clone()
        .oneshot(Request::get("/r/abc").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FOUND);
    assert!(
        res.headers()["location"]
            .to_str()
            .unwrap()
            .contains("bf_click=")
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM clicks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // Recovers once the guard clears.
    t.state.spool.set_shedding(false);
    let res = app.clone().oneshot(beacon()).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    std::fs::remove_dir_all(d).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn metrics_report_shedding_gauge(pool: PgPool) {
    let (t, app, d) = up(&pool).await;
    t.state.spool.set_shedding(true);
    app.clone().oneshot(beacon()).await.unwrap();
    tracker::metrics::spool_report(1234, 99, true);
    let m = tracker::metrics::render();
    assert!(m.contains("bf_spool_shedding 1"), "{m}");
    assert!(m.contains("bf_spool_free_bytes 1234"));
    assert!(m.contains("bf_spool_shed_total{route=\"c\"}"));
    tracker::metrics::spool_report(0, 0, false);
    std::fs::remove_dir_all(d).ok();
}
