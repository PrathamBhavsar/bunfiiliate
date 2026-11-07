//! B07 notify channels: Telegram/Discord URLs sealed at rest and redacted on read, fan-out for the
//! default topics only, and an immediate test message.

use std::sync::{Arc, Mutex};

use api::auth::{Principal, issue_access};
use api::dispatch::dispatch_outbox_notify;
use api::mail::Mailer;
use api::notify_channels::Channels;
use api::{AppState, app};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{Method, Request, StatusCode};
use axum::routing::post;
use bf_core::secrets::{KeyRing, Vault};
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-secret";
const TOKEN: &str = "bot123456:SECRET-TOKEN-XYZ";

fn vault() -> Arc<Vault> {
    let ring = KeyRing::from_pairs("t1", &[("t1", [5u8; 32])]).unwrap();
    Arc::new(Vault::new(ring))
}

struct Api(Router);

impl Api {
    fn new(pool: &PgPool) -> Self {
        api::ssrf::allow_private_targets();
        Self(app(AppState {
            pool: pool.clone(),
            jwt_keys: api::auth::JwtKeys::single(SECRET).into(),
            version_cache: Default::default(),
            tracker_url: Default::default(),
            vault: vault(),
            secure_cookies: true,
            limiter: Default::default(),
        }))
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        cookie: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method.clone())
            .uri(uri)
            .header(HOST, "bf.test")
            .header(COOKIE, format!("{cookie}; bf_csrf=t"));
        if method != Method::GET {
            b = b
                .header(ORIGIN, "https://bf.test")
                .header("x-csrf-token", "t");
        }
        let body = match body {
            Some(v) => {
                b = b.header(CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self.0.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

struct Tenant {
    ws: Uuid,
    admin: String,
    program: Uuid,
}

async fn tenant(pool: &PgPool) -> Tenant {
    let ws = Uuid::now_v7();
    create_workspace(pool, ws, "w", &ws.to_string())
        .await
        .unwrap();
    let u = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: format!("{}@x.io", Uuid::now_v7()),
        password_hash: "unused".into(),
        role: UserRole::Admin,
    };
    create_user(pool, &u).await.unwrap();
    let admin = format!(
        "bf_at={}",
        issue_access(SECRET, &Principal::from(&u)).unwrap()
    );
    let program = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
         VALUES ($1, $2, 'p', 'USD', $3, 's')",
    )
    .bind(program)
    .bind(ws)
    .bind(program.to_string())
    .execute(pool)
    .await
    .unwrap();
    Tenant { ws, admin, program }
}

type Seen = Arc<Mutex<Vec<String>>>;

async fn hook(State(seen): State<Seen>, body: String) -> StatusCode {
    seen.lock().unwrap().push(body);
    StatusCode::NO_CONTENT
}

/// Receiver on an ephemeral port; returns (url with a token-like path, bodies seen).
async fn receiver() -> (String, Seen) {
    let seen = Seen::default();
    let app = Router::new()
        .route("/hook/{token}", post(hook))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook/{TOKEN}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

async fn outbox(pool: &PgPool, topic: &str, payload: Value) {
    sqlx::query("INSERT INTO outbox (topic, payload) VALUES ($1, $2::text::jsonb)")
        .bind(topic)
        .bind(payload.to_string())
        .execute(pool)
        .await
        .unwrap();
}

fn channels() -> Channels {
    Channels::new(api::dispatch::http_client(), vault())
}

/// Wait (bounded) for sends spawned by the dispatcher.
async fn settle(seen: &Seen, want: usize) {
    for _ in 0..50 {
        if seen.lock().unwrap().len() >= want {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // Extra grace so an unwanted send would show up.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}

const URL: &str = "/api/v1/settings/notify-channels";

#[sqlx::test(migrations = "../db/migrations")]
async fn notify_channel_urls_are_sealed_and_redacted(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool).await;
    let (st, body) = api
        .call(
            Method::PUT,
            URL,
            &t.admin,
            Some(json!({"telegram_url": format!("https://api.telegram.org/{TOKEN}/sendMessage")})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"telegram_set": true, "discord_set": false}));

    let (raw, marker): (String, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT notify_telegram_webhook, notify_enc_at FROM workspaces WHERE id = $1",
    )
    .bind(t.ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(raw.starts_with("bfenc1:") && !raw.contains(TOKEN) && marker.is_some());
    let opened = vault()
        .open_col("workspaces", "notify_telegram_webhook", t.ws, &raw)
        .unwrap();
    assert!(opened.contains(TOKEN));

    let (st, got) = api.call(Method::GET, URL, &t.admin, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!got.to_string().contains(TOKEN), "{got}");
    // The generic settings blob must not carry it either.
    let (_, s) = api
        .call(Method::GET, "/api/v1/settings", &t.admin, None)
        .await;
    assert!(!s.to_string().contains(TOKEN));

    // null clears it
    let (st, body) = api
        .call(
            Method::PUT,
            URL,
            &t.admin,
            Some(json!({"telegram_url": null})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["telegram_set"], false);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn notify_channels_are_in_the_protected_list(_pool: PgPool) {
    for col in ["notify_telegram_webhook", "notify_discord_webhook"] {
        assert!(api::secrets::PROTECTED.contains(&("workspaces", col)));
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn telegram_webhook_sends_plain_text_on_payout_paid(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool).await;
    let (url, seen) = receiver().await;
    let (st, b) = api
        .call(
            Method::PUT,
            URL,
            &t.admin,
            Some(json!({"telegram_url": url})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let payout = Uuid::now_v7();
    outbox(
        &pool,
        "payout.paid",
        json!({"program_id": t.program, "payout_id": payout, "amount_cents": 1234, "currency": "USD"}),
    )
    .await;
    dispatch_outbox_notify(&pool, &Mailer::Log, &channels())
        .await
        .unwrap();
    settle(&seen, 1).await;
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "{got:?}");
    let v: Value = serde_json::from_str(&got[0]).unwrap();
    let text = v["text"].as_str().expect("telegram body has text");
    assert!(text.contains(&payout.to_string()), "{text}");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn discord_webhook_respects_topic_filter(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool).await;
    let (url, seen) = receiver().await;
    let (st, _) = api
        .call(
            Method::PUT,
            URL,
            &t.admin,
            Some(json!({"discord_url": url})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    outbox(
        &pool,
        "conversion.created",
        json!({"program_id": t.program, "conversion_id": Uuid::now_v7()}),
    )
    .await;
    dispatch_outbox_notify(&pool, &Mailer::Log, &channels())
        .await
        .unwrap();
    settle(&seen, 1).await;
    assert!(seen.lock().unwrap().is_empty());

    outbox(
        &pool,
        "conversion.reversed",
        json!({"program_id": t.program, "conversion_id": Uuid::now_v7(), "reason": "chargeback"}),
    )
    .await;
    dispatch_outbox_notify(&pool, &Mailer::Log, &channels())
        .await
        .unwrap();
    settle(&seen, 1).await;
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    let v: Value = serde_json::from_str(&got[0]).unwrap();
    assert!(
        v["content"]
            .as_str()
            .is_some_and(|c| c.contains("chargeback"))
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn other_workspaces_channels_are_not_used(pool: PgPool) {
    let api = Api::new(&pool);
    let a = tenant(&pool).await;
    let b = tenant(&pool).await;
    let (url, seen) = receiver().await;
    api.call(
        Method::PUT,
        URL,
        &a.admin,
        Some(json!({"discord_url": url})),
    )
    .await;
    outbox(
        &pool,
        "payout.paid",
        json!({"program_id": b.program, "payout_id": Uuid::now_v7()}),
    )
    .await;
    dispatch_outbox_notify(&pool, &Mailer::Log, &channels())
        .await
        .unwrap();
    settle(&seen, 1).await;
    assert!(seen.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn test_notify_channel_endpoint_sends_without_waiting_for_real_event(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool).await;
    let tc = "/api/v1/settings/test-notify-channel";
    // Not configured yet.
    let (st, _) = api
        .call(
            Method::POST,
            tc,
            &t.admin,
            Some(json!({"channel": "discord"})),
        )
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    let (url, seen) = receiver().await;
    api.call(
        Method::PUT,
        URL,
        &t.admin,
        Some(json!({"discord_url": url})),
    )
    .await;
    let (st, body) = api
        .call(
            Method::POST,
            tc,
            &t.admin,
            Some(json!({"channel": "discord"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(seen.lock().unwrap().len(), 1);

    let (st, _) = api
        .call(Method::POST, tc, &t.admin, Some(json!({"channel": "sms"})))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}
