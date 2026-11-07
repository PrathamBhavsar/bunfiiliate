use std::sync::Arc;

use api::auth::hash_password;
use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{Request, StatusCode};
use bf_core::billing::sign;
use bf_core::secrets::{KeyRing, Vault};
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct horse";
const SECRET: &str = "whsec_test_secret_0123456789";

fn vault() -> Arc<Vault> {
    let ring = KeyRing::from_pairs("t1", &[("t1", [7u8; 32])]).unwrap();
    Arc::new(Vault::new(ring))
}

fn app(pool: PgPool) -> Router {
    let state = AppState {
        pool,
        jwt_keys: api::auth::JwtKeys::single("test-secret").into(),
        version_cache: Default::default(),
        vault: vault(),
        secure_cookies: true,
        limiter: Default::default(),
    };
    build(routes::routes(), state)
}

struct Tenant {
    ws: Uuid,
    cookie: String,
}

async fn tenant(pool: &PgPool, app: &Router, role: UserRole) -> Tenant {
    let ws = Uuid::now_v7();
    create_workspace(pool, ws, "w", &ws.to_string())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO subscriptions (workspace_id, plan_id) \
         SELECT $1, id FROM billing_plans WHERE is_default ON CONFLICT DO NOTHING",
    )
    .bind(ws)
    .execute(pool)
    .await
    .unwrap();
    let email = format!("{ws}@x.io");
    let user = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: email.clone(),
        password_hash: hash_password(PASSWORD).unwrap(),
        role,
    };
    create_user(pool, &user).await.unwrap();
    let res = raw(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        json!({"email": email, "password": PASSWORD}),
        &[],
    )
    .await;
    assert_eq!(res.0, StatusCode::OK);
    let cookie = res
        .2
        .iter()
        .find(|c| c.starts_with("bf_at="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    Tenant { ws, cookie }
}

/// (status, body, set-cookie values)
async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
    extra: &[(&str, String)],
) -> (StatusCode, Value, Vec<String>) {
    let csrf = "bf_csrf=csrf-test-token";
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(HOST, "bf.test")
        .header(ORIGIN, "https://bf.test")
        .header(
            COOKIE,
            match cookie {
                Some(c) => format!("{c}; {csrf}"),
                None => csrf.to_string(),
            },
        );
    if method != "GET" {
        b = b.header("x-csrf-token", "csrf-test-token");
    }
    for (k, v) in extra {
        b = b.header(*k, v);
    }
    let req = b
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let st = res.status();
    let cookies = res
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        st,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookies,
    )
}

async fn call(
    app: &Router,
    t: &Tenant,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let r = raw(app, method, uri, Some(&t.cookie), body, &[]).await;
    (r.0, r.1)
}

/// Store the provider webhook secret the way the API does.
async fn enable_provider(pool: &PgPool, provider: &str) {
    let id = Uuid::now_v7();
    let sealed = vault().seal_col("billing_providers", "webhook_secret", id, SECRET);
    sqlx::query(
        "INSERT INTO billing_providers (id, provider, webhook_secret, secret_enc_at) \
         VALUES ($1, $2, $3, now())",
    )
    .bind(id)
    .bind(provider)
    .bind(sealed)
    .execute(pool)
    .await
    .unwrap();
}

async fn add_plan(pool: &PgPool, key: &str, max_programs: Option<i32>, max_aff: Option<i32>) {
    sqlx::query(
        "INSERT INTO billing_plans (id, key, name, price_cents, max_programs, max_affiliates) \
         VALUES ($1, $2, $2, 1000, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(key)
    .bind(max_programs)
    .bind(max_aff)
    .execute(pool)
    .await
    .unwrap();
}

async fn sub(pool: &PgPool, ws: Uuid) -> (String, String, String) {
    sqlx::query_as(
        "SELECT p.key, s.status::text, s.provider FROM subscriptions s \
         JOIN billing_plans p ON p.id = s.plan_id WHERE s.workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn event(ws: Uuid, id: Option<&str>, status: &str, updated: &str, plan: &str) -> String {
    let wid = id.map_or(String::new(), |i| format!(r#""webhook_id":"{i}","#));
    format!(
        r#"{{"meta":{{"event_name":"subscription_updated",{wid}"custom_data":{{"workspace_id":"{ws}","plan_key":"{plan}"}}}},"data":{{"id":"sub_9","attributes":{{"status":"{status}","updated_at":"{updated}","renews_at":"2027-01-01T00:00:00Z","ends_at":"2026-12-01T00:00:00Z"}}}}}}"#
    )
}

async fn hook(
    app: &Router,
    provider: &str,
    body: &str,
    sig: Option<String>,
) -> (StatusCode, Value) {
    // No Origin, no cookies: a real provider call.
    let mut b = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/webhooks/billing/{provider}"))
        .header(HOST, "bf.test");
    if let Some(s) = sig {
        b = b.header("x-signature", s);
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let st = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn signed(body: &str) -> Option<String> {
    Some(sign(SECRET, body.as_bytes()))
}

#[sqlx::test(migrations = "../db/migrations")]
async fn checkout_mock_activates_plan_immediately(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "pro", None, None).await;
    enable_provider(&pool, "mock").await;
    let (st, b) = call(
        &app,
        &t,
        "POST",
        "/api/v1/billing/checkout",
        json!({"plan_key": "pro"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["activated"], true);
    assert_eq!(
        sub(&pool, t.ws).await,
        ("pro".into(), "active".into(), "mock".into())
    );
    let (st, v) = call(&app, &t, "GET", "/api/v1/billing/subscription", json!({})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["plan"]["key"], "pro");
    assert_eq!(v["usage"]["programs"], 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn checkout_mock_requires_super_admin_enablement(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "pro", None, None).await;
    let (st, _) = call(
        &app,
        &t,
        "POST",
        "/api/v1/billing/checkout",
        json!({"plan_key": "pro"}),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(sub(&pool, t.ws).await.0, "free");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn checkout_unconfigured_provider_422(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "pro", None, None).await;
    let body = json!({"plan_key": "pro", "provider": "lemonsqueezy"});
    let (st, _) = call(&app, &t, "POST", "/api/v1/billing/checkout", body).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(sub(&pool, t.ws).await.0, "free");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn checkout_rejects_unknown_provider_and_plan(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    let (st, _) = call(
        &app,
        &t,
        "POST",
        "/api/v1/billing/checkout",
        json!({"plan_key": "free", "provider": "stripe"}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(
        &app,
        &t,
        "POST",
        "/api/v1/billing/checkout",
        json!({"plan_key": "nope"}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn billing_webhook_updates_subscription_status(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "pro", None, None).await;
    enable_provider(&pool, "lemonsqueezy").await;
    let b = event(t.ws, Some("e1"), "active", "2026-03-01T00:00:00Z", "pro");
    let (st, r) = hook(&app, "lemonsqueezy", &b, signed(&b)).await;
    assert_eq!(
        (st, r["result"].as_str()),
        (StatusCode::OK, Some("applied"))
    );
    assert_eq!(
        sub(&pool, t.ws).await,
        ("pro".into(), "active".into(), "lemonsqueezy".into())
    );

    let b = event(t.ws, Some("e2"), "cancelled", "2026-03-02T00:00:00Z", "pro");
    let (st, _) = hook(&app, "lemonsqueezy", &b, signed(&b)).await;
    assert_eq!(st, StatusCode::OK);
    // Canceled drops back to the default plan.
    assert_eq!(
        sub(&pool, t.ws).await,
        ("free".into(), "canceled".into(), "lemonsqueezy".into())
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn billing_webhook_bad_signature_400_no_write(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "pro", None, None).await;
    enable_provider(&pool, "lemonsqueezy").await;
    let b = event(t.ws, Some("e1"), "active", "2026-03-01T00:00:00Z", "pro");
    let tampered = b.replace("pro", "max");
    for sig in [signed(&b), Some("00".into()), None] {
        let (st, _) = hook(&app, "lemonsqueezy", &tampered, sig).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    assert_eq!(sub(&pool, t.ws).await.0, "free");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM billing_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn billing_webhook_dedupes_by_event_id(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "pro", None, None).await;
    enable_provider(&pool, "lemonsqueezy").await;
    let b = event(t.ws, Some("same"), "active", "2026-03-01T00:00:00Z", "pro");
    let (_, r) = hook(&app, "lemonsqueezy", &b, signed(&b)).await;
    assert_eq!(r["result"], "applied");
    // Same event id with a newer, different body: still a replay.
    let b2 = event(
        t.ws,
        Some("same"),
        "cancelled",
        "2026-04-01T00:00:00Z",
        "pro",
    );
    let (st, r) = hook(&app, "lemonsqueezy", &b2, signed(&b2)).await;
    assert_eq!(
        (st, r["result"].as_str()),
        (StatusCode::OK, Some("duplicate"))
    );
    assert_eq!(sub(&pool, t.ws).await.1, "active");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn billing_webhook_applies_in_updated_at_order(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "pro", None, None).await;
    enable_provider(&pool, "lemonsqueezy").await;
    // The newer event arrives first.
    let newer = event(t.ws, Some("b"), "past_due", "2026-05-01T00:00:00Z", "pro");
    let (_, r) = hook(&app, "lemonsqueezy", &newer, signed(&newer)).await;
    assert_eq!(r["result"], "applied");
    let older = event(t.ws, Some("a"), "active", "2026-04-01T00:00:00Z", "pro");
    let (st, r) = hook(&app, "lemonsqueezy", &older, signed(&older)).await;
    assert_eq!(
        (st, r["result"].as_str()),
        (StatusCode::OK, Some("ignored"))
    );
    assert_eq!(sub(&pool, t.ws).await.1, "past_due");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn billing_webhook_only_touches_its_workspace(pool: PgPool) {
    let app = app(pool.clone());
    let a = tenant(&pool, &app, UserRole::Admin).await;
    let b = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "pro", None, None).await;
    enable_provider(&pool, "lemonsqueezy").await;
    let body = event(a.ws, Some("e1"), "active", "2026-03-01T00:00:00Z", "pro");
    hook(&app, "lemonsqueezy", &body, signed(&body)).await;
    assert_eq!(sub(&pool, a.ws).await.0, "pro");
    assert_eq!(sub(&pool, b.ws).await.0, "free");
    // Unknown workspace: acknowledged, nothing created.
    let ghost = event(
        Uuid::now_v7(),
        Some("e2"),
        "active",
        "2026-03-01T00:00:00Z",
        "pro",
    );
    let (st, r) = hook(&app, "lemonsqueezy", &ghost, signed(&ghost)).await;
    assert_eq!(
        (st, r["result"].as_str()),
        (StatusCode::OK, Some("ignored"))
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn billing_webhook_provider_allowlist(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    enable_provider(&pool, "lemonsqueezy").await;
    let b = event(t.ws, Some("e1"), "active", "2026-03-01T00:00:00Z", "free");
    for p in ["stripe", "paddle", "LemonSqueezy", "mock"] {
        // `mock` is allowed but has no secret configured here.
        let (st, _) = hook(&app, p, &b, signed(&b)).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{p}");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn over_plan_limit_create_program_returns_402(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "tiny", Some(1), Some(1)).await;
    sqlx::query("UPDATE subscriptions SET plan_id = (SELECT id FROM billing_plans WHERE key = 'tiny') WHERE workspace_id = $1")
        .bind(t.ws)
        .execute(&pool)
        .await
        .unwrap();
    let new = json!({"name": "P", "currency": "USD"});
    let (st, b) = call(&app, &t, "POST", "/api/v1/programs", new.clone()).await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    let (st, b) = call(&app, &t, "POST", "/api/v1/programs", new).await;
    assert_eq!(st, StatusCode::PAYMENT_REQUIRED);
    assert!(
        b["error"]["message"]
            .as_str()
            .unwrap()
            .contains("max_programs"),
        "{b}"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn over_plan_limit_register_affiliate_returns_402(pool: PgPool) {
    let app = app(pool.clone());
    let t = tenant(&pool, &app, UserRole::Admin).await;
    add_plan(&pool, "tiny", None, Some(0)).await;
    sqlx::query("UPDATE subscriptions SET plan_id = (SELECT id FROM billing_plans WHERE key = 'tiny') WHERE workspace_id = $1")
        .bind(t.ws)
        .execute(&pool)
        .await
        .unwrap();
    let body = json!({"workspace": t.ws.to_string(), "email": "a@x.io", "password": "a long enough password"});
    let (st, b, _) = raw(&app, "POST", "/api/v1/affiliates/register", None, body, &[]).await;
    assert_eq!(st, StatusCode::PAYMENT_REQUIRED);
    assert!(
        b["error"]["message"]
            .as_str()
            .unwrap()
            .contains("max_affiliates"),
        "{b}"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn provider_secret_is_sealed_and_never_returned(pool: PgPool) {
    let app = app(pool.clone());
    let root = tenant(&pool, &app, UserRole::SuperAdmin).await;
    let admin = tenant(&pool, &app, UserRole::Admin).await;
    let uri = "/api/v1/billing/providers/lemonsqueezy";
    let body = json!({"webhook_secret": SECRET});
    // Only a super admin may set it; bad provider and short secret are rejected.
    let (st, _) = call(&app, &admin, "PUT", uri, body.clone()).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(
        &app,
        &root,
        "PUT",
        "/api/v1/billing/providers/stripe",
        body.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(&app, &root, "PUT", uri, json!({"webhook_secret": "short"})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, put) = call(&app, &root, "PUT", uri, body).await;
    assert_eq!(st, StatusCode::OK);
    let (st, list) = call(&app, &root, "GET", "/api/v1/billing/providers", json!({})).await;
    assert_eq!(st, StatusCode::OK);
    for v in [put.to_string(), list.to_string()] {
        assert!(!v.contains(SECRET) && !v.contains("bfenc1"), "{v}");
    }
    assert_eq!(list[0]["provider"], "lemonsqueezy");
    assert_eq!(list[0]["webhook_secret_set"], true);
    // At rest: sealed, marker set.
    let (stored, marker): (String, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT webhook_secret, secret_enc_at FROM billing_providers WHERE provider = 'lemonsqueezy'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(stored.starts_with("bfenc1:") && !stored.contains(SECRET));
    assert!(marker.is_some());
    // And the stored secret verifies a webhook end to end.
    add_plan(&pool, "pro", None, None).await;
    let b = event(
        admin.ws,
        Some("e1"),
        "active",
        "2026-03-01T00:00:00Z",
        "pro",
    );
    let (st, r) = hook(&app, "lemonsqueezy", &b, signed(&b)).await;
    assert_eq!(
        (st, r["result"].as_str()),
        (StatusCode::OK, Some("applied"))
    );
}
