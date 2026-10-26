use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bf_spool::{SpoolConfig, SpoolWriter};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tower::ServiceExt;
use tracker::geo::Geo;
use tracker::snapshot::Snapshot;
use tracker::{AppState, router, start};
use uuid::Uuid;

const SALT: &str = "test-salt";
const GENERIC: &str = "generic-secret";
const STRIPE: &str = "whsec_stripe";
const PADDLE: &str = "pdl_secret";

fn spool_dir() -> PathBuf {
    std::env::temp_dir().join(format!("bf-webhook-test-{}", Uuid::now_v7()))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A program `pk` with all three secrets, and one without Stripe/Paddle (`bare`).
async fn seed(pool: &PgPool) {
    let ws = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', 'w')")
        .bind(ws)
        .execute(pool)
        .await
        .unwrap();
    for (key, stripe, paddle) in [("pk", Some(STRIPE), Some(PADDLE)), ("bare", None, None)] {
        sqlx::query(
            "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret, \
                                   stripe_secret, paddle_secret) \
             VALUES ($1, $2, $3, 'USD', $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(ws)
        .bind(key)
        .bind(GENERIC)
        .bind(stripe)
        .bind(paddle)
        .execute(pool)
        .await
        .unwrap();
    }
}

fn hex_hmac(secret: &str, parts: &[&[u8]]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    parts.iter().for_each(|p| mac.update(p));
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn post(path: &str, headers: &[(&str, String)], body: &str) -> Request<Body> {
    let mut req = Request::post(path).header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    req.body(Body::from(body.to_owned())).unwrap()
}

fn generic(key: &str, body: &str) -> Request<Body> {
    let sig = hex_hmac(GENERIC, &[body.as_bytes()]);
    post(
        "/v1/events/pk",
        &[("x-bf-signature", sig), ("idempotency-key", key.to_owned())],
        body,
    )
}

fn stripe_at(ts: i64, id: &str) -> Request<Body> {
    let body = format!(r#"{{"id":"{id}","type":"invoice.paid","data":{{"amount_paid":4900}}}}"#);
    let sig = hex_hmac(STRIPE, &[ts.to_string().as_bytes(), b".", body.as_bytes()]);
    let header = format!("t={ts},v1={sig}");
    post(
        "/v1/webhooks/stripe/pk",
        &[("stripe-signature", header)],
        &body,
    )
}

fn paddle_at(ts: i64, id: &str) -> Request<Body> {
    let body = format!(r#"{{"event_id":"{id}","event_type":"transaction.completed"}}"#);
    let sig = hex_hmac(PADDLE, &[ts.to_string().as_bytes(), b":", body.as_bytes()]);
    let header = format!("ts={ts};h1={sig}");
    post(
        "/v1/webhooks/paddle/pk",
        &[("paddle-signature", header)],
        &body,
    )
}

async fn status(app: &Router, req: Request<Body>) -> StatusCode {
    app.clone().oneshot(req).await.unwrap().status()
}

async fn events(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM inbound_events")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn wait_events(pool: &PgPool, n: i64) {
    for _ in 0..200 {
        if events(pool).await == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "inbound_events never reached {n}, now {}",
        events(pool).await
    );
}

fn spool_end(dir: &Path) -> u64 {
    bf_spool::replay(dir, 0)
        .unwrap()
        .map(|r| r.end_offset())
        .last()
        .unwrap_or(0)
}

/// State with a queue nobody drains: records get spooled but never reach Postgres.
async fn undrained(
    pool: &PgPool,
    dir: &Path,
) -> (AppState, mpsc::Receiver<tracker::writer::Queued>) {
    let (queue, rx) = mpsc::channel(100);
    let state = AppState {
        pool: pool.clone(),
        snapshot: Arc::new(ArcSwap::from_pointee(Snapshot::load(pool).await.unwrap())),
        spool: SpoolWriter::open(SpoolConfig::new(dir)).await.unwrap(),
        queue,
        ip_salt: SALT.into(),
        geo: Arc::new(Geo::disabled()),
    };
    (state, rx)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_valid_signatures_are_stored(pool: PgPool) {
    seed(&pool).await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    let app = router(t.state.clone());

    assert_eq!(
        status(&app, generic("order-1", r#"{"amount_cents":4900}"#)).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        status(&app, stripe_at(now(), "evt_s1")).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, paddle_at(now(), "evt_p1")).await,
        StatusCode::OK
    );
    wait_events(&pool, 3).await;

    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT source::text, external_id, \
                COALESCE((payload #>> '{amount_cents}')::bigint, (payload #>> '{data,amount_paid}')::bigint, 0) \
         FROM inbound_events ORDER BY external_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            ("paddle".to_owned(), "evt_p1".to_owned(), 0),
            ("stripe".to_owned(), "evt_s1".to_owned(), 4900),
            ("generic".to_owned(), "order-1".to_owned(), 4900),
        ]
    );
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inbound_events WHERE processed_at IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pending, 3);
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_bad_requests_are_400_and_spool_nothing(pool: PgPool) {
    seed(&pool).await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    let app = router(t.state.clone());
    let bad = StatusCode::BAD_REQUEST;

    // Tampered body: signature was made over a different payload.
    let mut req = generic("k1", r#"{"a":1}"#);
    *req.body_mut() = Body::from(r#"{"a":2}"#);
    assert_eq!(status(&app, req).await, bad);
    // Wrong signature on each source.
    let wrong = "0".repeat(64);
    assert_eq!(
        status(
            &app,
            post(
                "/v1/events/pk",
                &[
                    ("x-bf-signature", wrong.clone()),
                    ("idempotency-key", "k".into())
                ],
                "{}"
            )
        )
        .await,
        bad
    );
    let ts = now();
    for path in ["/v1/webhooks/stripe/pk", "/v1/webhooks/paddle/pk"] {
        let (name, value) = if path.contains("stripe") {
            ("stripe-signature", format!("t={ts},v1={wrong}"))
        } else {
            ("paddle-signature", format!("ts={ts};h1={wrong}"))
        };
        assert_eq!(
            status(
                &app,
                post(path, &[(name, value)], r#"{"id":"e","event_id":"e"}"#)
            )
            .await,
            bad
        );
    }
    // Missing signature or Idempotency-Key.
    assert_eq!(status(&app, post("/v1/events/pk", &[], "{}")).await, bad);
    let sig = hex_hmac(GENERIC, &[b"{}"]);
    assert_eq!(
        status(
            &app,
            post("/v1/events/pk", &[("x-bf-signature", sig)], "{}")
        )
        .await,
        bad
    );
    // Signed but not usable: not JSON, NUL escape, no event id.
    for body in ["not json", r#"{"a":"\u0000"}"#, r#"{"a":1}"#] {
        let sig = hex_hmac(STRIPE, &[ts.to_string().as_bytes(), b".", body.as_bytes()]);
        let h = [("stripe-signature", format!("t={ts},v1={sig}"))];
        assert_eq!(
            status(&app, post("/v1/webhooks/stripe/pk", &h, body)).await,
            bad,
            "{body}"
        );
    }
    // No Stripe secret configured for the program; unknown program.
    let mut req = stripe_at(ts, "evt");
    *req.uri_mut() = "/v1/webhooks/stripe/bare".parse().unwrap();
    assert_eq!(status(&app, req).await, bad);
    let mut req = generic("k", "{}");
    *req.uri_mut() = "/v1/events/nope".parse().unwrap();
    assert_eq!(status(&app, req).await, StatusCode::NOT_FOUND);

    assert_eq!(bf_spool::replay(&dir, 0).unwrap().count(), 0);
    assert_eq!(events(&pool).await, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_replay_beyond_tolerance_is_400(pool: PgPool) {
    seed(&pool).await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    let app = router(t.state.clone());

    for skew in [-301, 301, 3600] {
        assert_eq!(
            status(&app, stripe_at(now() - skew, "evt_old")).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(&app, paddle_at(now() - skew, "evt_old")).await,
            StatusCode::BAD_REQUEST
        );
    }
    // Inside the window it is accepted.
    assert_eq!(
        status(&app, stripe_at(now() - 200, "evt_ok")).await,
        StatusCode::OK
    );
    wait_events(&pool, 1).await;
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_duplicate_delivery_is_one_row(pool: PgPool) {
    seed(&pool).await;
    let dir = spool_dir();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    let app = router(t.state.clone());

    // Concurrent redeliveries of the same Stripe event and Paddle event, a generic
    // retry whose body differs but whose Idempotency-Key repeats.
    let mut set = JoinSet::new();
    for i in 0..10 {
        let app = app.clone();
        set.spawn(async move {
            let reqs = [
                stripe_at(now() - i, "evt_dup"),
                paddle_at(now(), "evt_dup"),
                generic("order-9", &format!(r#"{{"try":{i}}}"#)),
            ];
            for req in reqs {
                assert!(
                    app.clone()
                        .oneshot(req)
                        .await
                        .unwrap()
                        .status()
                        .is_success()
                );
            }
        });
    }
    while let Some(r) = set.join_next().await {
        r.unwrap();
    }
    wait_events(&pool, 3).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(events(&pool).await, 3);
    std::fs::remove_dir_all(&dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_crash_and_replay_is_one_row(pool: PgPool) {
    seed(&pool).await;
    let dir = spool_dir();

    // Run 1: acked (spooled) but the process dies before the DB flush.
    {
        let (state, _rx) = undrained(&pool, &dir).await;
        let app = router(state);
        assert_eq!(
            status(&app, stripe_at(now(), "evt_crash")).await,
            StatusCode::OK
        );
        assert_eq!(
            status(&app, generic("order-crash", "{}")).await,
            StatusCode::ACCEPTED
        );
        assert_eq!(events(&pool).await, 0);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Run 2: startup replays the spool.
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    assert_eq!(events(&pool).await, 2);
    assert_eq!(t.state.spool.read_checkpoint().unwrap(), spool_end(&dir));
    drop(t);

    // DB flush done but checkpoint lost: replay again, still one row each; and the
    // provider redelivering after the restart adds nothing.
    bf_spool::write_checkpoint(&dir, 0).unwrap();
    let t = start(pool.clone(), SpoolConfig::new(&dir), SALT, Geo::disabled())
        .await
        .unwrap();
    assert_eq!(events(&pool).await, 2);
    let app = router(t.state.clone());
    assert_eq!(
        status(&app, stripe_at(now(), "evt_crash")).await,
        StatusCode::OK
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(events(&pool).await, 2);
    std::fs::remove_dir_all(&dir).ok();
}
