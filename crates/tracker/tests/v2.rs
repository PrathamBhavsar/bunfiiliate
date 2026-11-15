//! `/v2` routes: strict validation, `Bf-Signature`, error bodies, v1/v2 header separation.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, Response, StatusCode};
use bf_spool::{SpoolConfig, SpoolWriter};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tower::ServiceExt;
use tracker::geo::Geo;
use tracker::snapshot::Snapshot;
use tracker::{AppState, router, start};
use uuid::Uuid;

const SECRET: &str = "generic-secret";
const OLD_SECRET: &str = "old-secret";
const STRIPE: &str = "whsec_stripe";

const SALE: &str = r#"{"schema":"bf.event.v2","type":"sale","external_id":"ord_1","occurred_at":"2026-01-01T12:00:00Z","amount_cents":4900,"currency":"USD"}"#;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
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

fn sig(secret: &str, t: i64, key: &str, body: &str) -> String {
    let t = t.to_string();
    hex_hmac(
        secret,
        &[t.as_bytes(), b".", key.as_bytes(), b".", body.as_bytes()],
    )
}

fn v2_at(t: i64, key: &str, body: &str) -> Request<Body> {
    let header = format!("t={t},v1={}", sig(SECRET, t, key, body));
    v2_with(&header, key, body)
}

fn v2_with(bf_sig: &str, key: &str, body: &str) -> Request<Body> {
    Request::post("/v2/events/pk")
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .header("bf-signature", bf_sig)
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn v2(key: &str, body: &str) -> Request<Body> {
    v2_at(now(), key, body)
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
    q("INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret, stripe_secret) \
       VALUES ($1, $2, 'p', 'USD', 'pk', $3, $4)")
        .bind(prog).bind(ws).bind(SECRET).bind(STRIPE).execute(pool).await.unwrap();
    q(
        "INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) \
       VALUES ('abc', $1, $2, 'https://example.com/pricing')",
    )
    .bind(aff)
    .bind(prog)
    .execute(pool)
    .await
    .unwrap();
}

async fn app(pool: &PgPool) -> (Router, PathBuf) {
    seed(pool).await;
    let dir = std::env::temp_dir().join(format!("bf-v2-test-{}", Uuid::now_v7()));
    let t = start(
        pool.clone(),
        SpoolConfig::new(&dir),
        "salt",
        Geo::disabled(),
        Arc::new(bf_core::secrets::Vault::plaintext()),
    )
    .await
    .unwrap();
    (router(t.state.clone()), dir)
}

async fn send(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

async fn error_code(res: Response<Body>) -> String {
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).expect("error body is JSON");
    assert!(v["error"]["message"].is_string() && v["error"]["request_id"].is_string());
    v["error"]["code"].as_str().unwrap().to_owned()
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn wait_count(pool: &PgPool, table: &str, n: i64) {
    for _ in 0..200 {
        if count(pool, table).await == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "{table} never reached {n}, now {}",
        count(pool, table).await
    );
}

/// Expect `status` and the schema-shaped error body; nothing may reach the database.
async fn rejected(app: &Router, pool: &PgPool, req: Request<Body>, status: StatusCode) -> String {
    let res = send(app, req).await;
    assert_eq!(res.status(), status);
    let code = error_code(res).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(count(pool, "inbound_events").await, 0);
    code
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_event_valid_202_and_stored(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let res = send(&app, v2("k-1", SALE)).await;
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let v: Value =
        serde_json::from_slice(&to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(v["status"], "accepted");
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    wait_count(&pool, "inbound_events", 1).await;
    let row: (Uuid, String, String, i64) = sqlx::query_as(
        "SELECT id, source::text, external_id, (payload->>'amount_cents')::bigint FROM inbound_events",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (id, "generic".into(), "k-1".into(), 4900));
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_missing_or_bad_idempotency_key_400(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let t = now();
    let header = format!("t={t},v1={}", sig(SECRET, t, "", SALE));
    let missing = Request::post("/v2/events/pk")
        .header("bf-signature", header)
        .body(Body::from(SALE))
        .unwrap();
    rejected(&app, &pool, missing, StatusCode::BAD_REQUEST).await;
    for bad in ["has space", "dot.dot", &"a".repeat(65)] {
        rejected(&app, &pool, v2(bad, SALE), StatusCode::BAD_REQUEST).await;
    }
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_signature_failures_401(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let t = now();
    let good = sig(SECRET, t, "k", SALE);
    // wrong, expired, future, malformed, missing, signed for another key or body
    let cases = [
        format!("t={t},v1={}", "0".repeat(64)),
        format!("t={},v1={}", t - 400, sig(SECRET, t - 400, "k", SALE)),
        format!("t={},v1={}", t + 400, sig(SECRET, t + 400, "k", SALE)),
        format!("t=abc,v1={good}"),
        format!("v1={good}"),
        format!("t={t}"),
        format!("t={t},v1=zz"),
        format!("t={t},v1={}", sig(SECRET, t, "other", SALE)),
        format!("t={t},v1={}", sig(SECRET, t, "k", "{}")),
    ];
    for c in &cases {
        let code = rejected(&app, &pool, v2_with(c, "k", SALE), StatusCode::UNAUTHORIZED).await;
        assert_eq!(code, "invalid_signature", "{c}");
    }
    let none = Request::post("/v2/events/pk")
        .header("idempotency-key", "k")
        .body(Body::from(SALE))
        .unwrap();
    rejected(&app, &pool, none, StatusCode::UNAUTHORIZED).await;
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_multi_v1_rotation_ok(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let t = now();
    let (old, new) = (sig(OLD_SECRET, t, "k", SALE), sig(SECRET, t, "k", SALE));
    // both orders verify; the same key and body is then a duplicate (200)
    for (header, want) in [
        (format!("t={t},v1={old},v1={new}"), StatusCode::ACCEPTED),
        (format!("t={t},v1={new},v1={old}"), StatusCode::OK),
    ] {
        assert_eq!(send(&app, v2_with(&header, "k", SALE)).await.status(), want);
    }
    let only_old = format!("t={t},v1={old}");
    rejected_after(
        &app,
        v2_with(&only_old, "k", SALE),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    std::fs::remove_dir_all(dir).ok();
}

async fn rejected_after(app: &Router, req: Request<Body>, status: StatusCode) {
    assert_eq!(send(app, req).await.status(), status);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_and_v1_header_schemes_do_not_cross(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let t = now();
    let legacy = |path: &str| {
        Request::post(path)
            .header("idempotency-key", "k")
            .header("x-bf-timestamp", t.to_string())
            .header("x-bf-signature", sig(SECRET, t, "k", SALE))
            .body(Body::from(SALE))
            .unwrap()
    };
    // v1 headers on a v2 route: 401. Bf-Signature on a v1 route: 400 (v1 status for bad sigs).
    rejected(
        &app,
        &pool,
        legacy("/v2/events/pk"),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let bf = format!("t={t},v1={}", sig(SECRET, t, "k", SALE));
    let on_v1 = Request::post("/v1/events/pk")
        .header("idempotency-key", "k")
        .header("bf-signature", bf)
        .body(Body::from(SALE))
        .unwrap();
    rejected_v1(&app, on_v1).await;
    // v1 still accepts v1 headers.
    assert_eq!(
        send(&app, legacy("/v1/events/pk")).await.status(),
        StatusCode::ACCEPTED
    );
    std::fs::remove_dir_all(dir).ok();
}

async fn rejected_v1(app: &Router, req: Request<Body>) {
    assert_eq!(send(app, req).await.status(), StatusCode::BAD_REQUEST);
}

fn sale_with(extra: &str) -> String {
    format!(
        r#"{{"schema":"bf.event.v2","type":"sale","external_id":"ord_1","occurred_at":"2026-01-01T12:00:00Z",{extra}}}"#
    )
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_validation_400s_spool_nothing(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let bad: Vec<(&str, String)> = vec![
        (
            "float",
            sale_with(r#""amount_cents":49.0,"currency":"USD""#),
        ),
        ("exp", sale_with(r#""amount_cents":4.9e3,"currency":"USD""#)),
        (
            "negative",
            sale_with(r#""amount_cents":-1,"currency":"USD""#),
        ),
        (
            "currency",
            sale_with(r#""amount_cents":1,"currency":"usd""#),
        ),
        (
            "nul",
            sale_with(r#""amount_cents":1,"currency":"USD","meta":{"a":"x\u0000y"}"#),
        ),
        (
            "unknown",
            sale_with(r#""amount_cents":1,"currency":"USD","email":"a@b.c""#),
        ),
        (
            "raw-email",
            sale_with(r#""amount_cents":1,"currency":"USD","customer":{"email":"a@b.c"}"#),
        ),
        ("no-amount", sale_with(r#""currency":"USD""#)),
        (
            "refund-no-ref",
            sale_with(r#""amount_cents":1,"currency":"USD","type":"refund""#)
                .replace(r#""type":"sale","#, ""),
        ),
        ("not-json", "nope".to_owned()),
        ("wrong-schema", SALE.replace("bf.event.v2", "bf.event.v1")),
    ];
    for (name, body) in &bad {
        let body = if *name == "refund-no-ref" {
            SALE.replace(r#""type":"sale""#, r#""type":"refund""#)
        } else {
            body.clone()
        };
        let code = rejected(&app, &pool, v2("k", &body), StatusCode::BAD_REQUEST).await;
        assert_eq!(code, "invalid_request", "{name}");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_oversize_413_json_body(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let big = "x".repeat(300 * 1024);
    let code = rejected(&app, &pool, v2("k", &big), StatusCode::PAYLOAD_TOO_LARGE).await;
    assert_eq!(code, "payload_too_large");
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_unknown_key_404(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let req = Request::post("/v2/events/nope")
        .header("idempotency-key", "k")
        .body(Body::from(SALE))
        .unwrap();
    assert_eq!(
        rejected(&app, &pool, req, StatusCode::NOT_FOUND).await,
        "unknown_program"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_stripe_webhook_signed_and_stored(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let body = r#"{"id":"evt_1","type":"invoice.paid"}"#;
    let t = now();
    let s = hex_hmac(STRIPE, &[t.to_string().as_bytes(), b".", body.as_bytes()]);
    let req = |sig: &str| {
        Request::post("/v2/webhooks/stripe/pk")
            .header("stripe-signature", format!("t={t},v1={sig}"))
            .body(Body::from(body))
            .unwrap()
    };
    assert_eq!(send(&app, req(&s)).await.status(), StatusCode::OK);
    wait_count(&pool, "inbound_events", 1).await;
    assert_eq!(
        send(&app, req(&"0".repeat(64))).await.status(),
        StatusCode::UNAUTHORIZED
    );
    std::fs::remove_dir_all(dir).ok();
}

fn beacon(body: &str) -> Request<Body> {
    Request::post("/v2/c")
        .header("content-type", "text/plain")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

const BEACON: &str =
    r#"{"v":2,"code":"abc","vid":"vid_abcdef123456","url":"https://shop.example/p"}"#;

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_beacon_204_and_one_click_row(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let res = send(&app, beacon(BEACON)).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    wait_count(&pool, "clicks", 1).await;
    // click-only is recorded by /r already: 204 with no new row
    let only = r#"{"v":2,"click":"01J0","vid":"vid_abcdef123456","url":"https://shop.example/p"}"#;
    assert_eq!(
        send(&app, beacon(only)).await.status(),
        StatusCode::NO_CONTENT
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(count(&pool, "clicks").await, 1);
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_beacon_rejections(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    for body in [
        "not json",
        r#"{"v":1,"code":"abc","vid":"vid_abcdef123456","url":"https://x.io/"}"#,
        r#"{"code":"abc","vid":"vid_abcdef123456","url":"https://x.io/"}"#,
        r#"{"v":2,"vid":"vid_abcdef123456","url":"https://x.io/"}"#,
        r#"{"v":2,"code":"abc","vid":"short","url":"https://x.io/"}"#,
        r#"{"v":2,"code":"abc","vid":"vid_abc def123","url":"https://x.io/"}"#,
        r#"{"v":2,"code":"abc","vid":"vid_abcdef123456","url":"ftp://x.io/"}"#,
        r#"{"v":2,"code":"abc","vid":"vid_abcdef123456","url":"https://x.io/","extra":1}"#,
        r#"{"v":2,"code":"abc","vid":"vid_abcdef123456","url":"https://x.io/","ref":"a\u0000b"}"#,
    ] {
        let res = send(&app, beacon(body)).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(error_code(res).await, "invalid_request");
    }
    let unknown = BEACON.replace("abc", "zzz");
    assert_eq!(
        send(&app, beacon(&unknown)).await.status(),
        StatusCode::NOT_FOUND
    );
    let big = format!(
        r#"{{"v":2,"code":"abc","vid":"vid_abcdef123456","url":"https://x.io/{}"}}"#,
        "a".repeat(5000)
    );
    let res = send(&app, beacon(&big)).await;
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_code(res).await, "payload_too_large");
    assert_eq!(count(&pool, "clicks").await, 0);
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_beacon_cors_preflight_headers(pool: PgPool) {
    let (app, dir) = app(&pool).await;
    let req = Request::options("/v2/c")
        .header("access-control-request-headers", "content-type")
        .body(Body::empty())
        .unwrap();
    let res = send(&app, req).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        res.headers()["access-control-allow-methods"],
        "POST, OPTIONS"
    );
    // errors carry CORS too, so a browser can read them
    let res = send(&app, beacon("nope")).await;
    assert_eq!(res.headers()["access-control-allow-origin"], "*");
    std::fs::remove_dir_all(dir).ok();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn v2_queue_full_503_with_retry_after(pool: PgPool) {
    seed(&pool).await;
    let dir = std::env::temp_dir().join(format!("bf-v2-test-{}", Uuid::now_v7()));
    let (queue, _rx) = mpsc::channel(1);
    let state = AppState {
        pool: pool.clone(),
        snapshot: Arc::new(ArcSwap::from_pointee(
            Snapshot::load(&pool, &bf_core::secrets::Vault::plaintext())
                .await
                .unwrap(),
        )),
        spool: SpoolWriter::open(SpoolConfig::new(&dir)).await.unwrap(),
        queue,
        ip_salt: "salt".into(),
        trust: Default::default(),
        geo: Arc::new(Geo::disabled()),
        fraud: bf_core::default_rules().into(),
        idem: Default::default(),
    };
    let app = router(state);
    assert_eq!(
        send(&app, v2("k1", SALE)).await.status(),
        StatusCode::ACCEPTED
    );
    let res = send(&app, v2("k2", SALE)).await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.headers()["retry-after"], "1");
    assert_eq!(error_code(res).await, "overloaded");
    std::fs::remove_dir_all(dir).ok();
}
