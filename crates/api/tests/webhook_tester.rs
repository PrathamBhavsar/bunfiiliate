//! B01: inbound webhook history, replay and the signed test sender.

use std::sync::{Arc, Mutex};

use api::auth::{Principal, issue_access};
use api::{AppState, TrackerUrl, app};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use bf_core::webhook_sign::{Layout, sign};
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-secret";

/// What the stub tracker saw: (path, headers, body).
type Seen = Arc<Mutex<Vec<(String, HeaderMap, Vec<u8>)>>>;

/// A stand-in tracker on loopback that records requests and answers `status`.
async fn stub_tracker(status: StatusCode) -> (String, Seen) {
    let seen: Seen = Arc::default();
    async fn any(
        State((seen, status)): State<(Seen, StatusCode)>,
        Path(rest): Path<String>,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, &'static str) {
        seen.lock().unwrap().push((rest, headers, body.to_vec()));
        (status, "stub-body")
    }
    let router = Router::new()
        .route("/{*rest}", axum::routing::post(any))
        .with_state((seen.clone(), status));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, seen)
}

struct Api(Router);

impl Api {
    fn new(pool: &PgPool, tracker: &str) -> Self {
        Self(app(AppState {
            pool: pool.clone(),
            jwt_keys: api::auth::JwtKeys::single(SECRET).into(),
            version_cache: Default::default(),
            vault: Arc::new(bf_core::secrets::Vault::plaintext()),
            secure_cookies: true,
            limiter: Default::default(),
            tracker_url: TrackerUrl(tracker.into()),
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
            .header(HOST, "bf.test");
        if method != Method::GET {
            b = b
                .header(ORIGIN, "https://bf.test")
                .header("x-csrf-token", "t");
        }
        let csrf = if method != Method::GET {
            "; bf_csrf=t"
        } else {
            ""
        };
        b = b.header(COOKIE, format!("{cookie}{csrf}"));
        let body = match body {
            Some(v) => {
                b = b.header(CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self.0.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let st = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

struct Tenant {
    ws: Uuid,
    program: Uuid,
    public_key: String,
    admin: String,
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
    let (program, public_key) = (Uuid::now_v7(), Uuid::now_v7().simple().to_string());
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret, \
           stripe_secret) VALUES ($1, $2, 'p', 'USD', $3, 'whsec_generic', 'whsec_stripe')",
    )
    .bind(program)
    .bind(ws)
    .bind(&public_key)
    .execute(pool)
    .await
    .unwrap();
    Tenant {
        ws,
        program,
        public_key,
        admin,
    }
}

fn hdr(h: &HeaderMap, k: &str) -> String {
    h.get(k)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_tester_generic_is_signed_with_the_program_secret(pool: PgPool) {
    let (url, seen) = stub_tracker(StatusCode::ACCEPTED).await;
    let api = Api::new(&pool, &url);
    let t = tenant(&pool).await;
    let payload = json!({"type": "sale", "customer": "c1", "amount_cents": 4900});
    let (st, body) = api
        .call(
            Method::POST,
            "/api/v1/webhooks/test",
            &t.admin,
            Some(json!({"program_id": t.program, "source": "generic", "payload": payload})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["status"], 202);
    assert_eq!(body["body"], "stub-body");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (path, headers, raw) = &seen[0];
    assert_eq!(path, &format!("v1/events/{}", t.public_key));
    assert_eq!(serde_json::from_slice::<Value>(raw).unwrap(), payload);
    // The signature is exactly what the tracker's verifier computes for this secret.
    let ts: i64 = hdr(headers, "x-bf-timestamp").parse().unwrap();
    let want = sign(
        Layout::Generic,
        "whsec_generic",
        &hdr(headers, "idempotency-key"),
        raw,
        ts,
    );
    assert_eq!(hdr(headers, "x-bf-signature"), want[0].1);
    assert!(hdr(headers, "idempotency-key").starts_with("test-"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_tester_stripe_and_missing_secret(pool: PgPool) {
    let (url, seen) = stub_tracker(StatusCode::OK).await;
    let api = Api::new(&pool, &url);
    let t = tenant(&pool).await;
    let payload = json!({"id": "evt_1", "type": "charge.refunded"});
    let (st, body) = api
        .call(
            Method::POST,
            "/api/v1/webhooks/test",
            &t.admin,
            Some(json!({"program_id": t.program, "source": "stripe", "payload": payload})),
        )
        .await;
    assert_eq!((st, body["status"].clone()), (StatusCode::OK, json!(200)));
    {
        let seen = seen.lock().unwrap();
        let (path, headers, raw) = &seen[0];
        assert_eq!(path, &format!("v1/webhooks/stripe/{}", t.public_key));
        let sig = hdr(headers, "stripe-signature");
        let ts: i64 = sig
            .split(',')
            .next()
            .unwrap()
            .trim_start_matches("t=")
            .parse()
            .unwrap();
        assert_eq!(sig, sign(Layout::Stripe, "whsec_stripe", "", raw, ts)[0].1);
    }
    // No Paddle secret on the program: 400, nothing sent.
    let (st, _) = api
        .call(
            Method::POST,
            "/api/v1/webhooks/test",
            &t.admin,
            Some(json!({"program_id": t.program, "source": "paddle", "payload": {}})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = api
        .call(
            Method::POST,
            "/api/v1/webhooks/test",
            &t.admin,
            Some(json!({"program_id": t.program, "source": "nope", "payload": {}})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_tester_bad_or_foreign_program_404_without_outbound_call(pool: PgPool) {
    let (url, seen) = stub_tracker(StatusCode::OK).await;
    let api = Api::new(&pool, &url);
    let (a, b) = (tenant(&pool).await, tenant(&pool).await);
    for program in [Uuid::now_v7(), b.program] {
        let (st, _) = api
            .call(
                Method::POST,
                "/api/v1/webhooks/test",
                &a.admin,
                Some(json!({"program_id": program, "source": "generic", "payload": {}})),
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_tester_unreachable_tracker_is_502(pool: PgPool) {
    // Port 1 on loopback refuses connections.
    let api = Api::new(&pool, "http://127.0.0.1:1");
    let t = tenant(&pool).await;
    let (st, _) = api
        .call(
            Method::POST,
            "/api/v1/webhooks/test",
            &t.admin,
            Some(json!({"program_id": t.program, "source": "generic", "payload": {}})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_GATEWAY);
}

async fn inbound(
    pool: &PgPool,
    t: &Tenant,
    source: &str,
    ext: &str,
    payload: Value,
    done: bool,
    error: Option<&str>,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO inbound_events (id, program_id, source, external_id, payload, processed_at, error) \
         VALUES ($1, $2, $3::event_source, $4, $5, CASE WHEN $6 THEN now() END, $7)",
    )
    .bind(id)
    .bind(t.program)
    .bind(source)
    .bind(ext)
    .bind(payload)
    .bind(done)
    .bind(error)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_tester_lists_inbound_scoped_and_filtered(pool: PgPool) {
    let api = Api::new(&pool, "http://127.0.0.1:1");
    let (a, b) = (tenant(&pool).await, tenant(&pool).await);
    inbound(&pool, &a, "generic", "g1", json!({}), true, None).await;
    inbound(&pool, &a, "stripe", "s1", json!({}), true, Some("boom")).await;
    inbound(&pool, &b, "generic", "other", json!({}), true, None).await;
    let (st, body) = api
        .call(Method::GET, "/api/v1/webhooks/inbound", &a.admin, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["total"], 2);
    let (_, body) = api
        .call(
            Method::GET,
            "/api/v1/webhooks/inbound?source=stripe",
            &a.admin,
            None,
        )
        .await;
    assert_eq!(body["items"][0]["error"], "boom");
    assert_eq!(body["total"], 1);
    // Another workspace's program id filters to nothing.
    let uri = format!("/api/v1/webhooks/inbound?program_id={}", b.program);
    let (_, body) = api.call(Method::GET, &uri, &a.admin, None).await;
    assert_eq!(body["total"], 0);
    let (st, _) = api
        .call(
            Method::GET,
            "/api/v1/webhooks/inbound?source=bogus",
            &a.admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn replay_inbound_reprocesses_without_reverify(pool: PgPool) {
    let api = Api::new(&pool, "http://127.0.0.1:1");
    let t = tenant(&pool).await;
    // An affiliate with a code, so a sale carrying it becomes a conversion.
    let (user, aff) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO users (id, workspace_id, email, password_hash, role) VALUES ($1, $2, $3, 'h', 'affiliate')")
        .bind(user).bind(t.ws).bind(format!("{user}@x.io")).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO affiliates (id, workspace_id, user_id, status) VALUES ($1, $2, $3, 'active')",
    )
    .bind(aff)
    .bind(t.ws)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO enrollments (affiliate_id, program_id) VALUES ($1, $2)")
        .bind(aff)
        .bind(t.program)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) VALUES ('REPLAY', $1, $2, 'https://x.io/')")
        .bind(aff).bind(t.program).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO commission_plans (id, program_id, kind, rate_bps, duration) VALUES ($1, $2, 'percent', 1000, 'lifetime')")
        .bind(Uuid::now_v7()).bind(t.program).execute(&pool).await.unwrap();
    sqlx::query("UPDATE programs SET default_plan_id = (SELECT id FROM commission_plans WHERE program_id = $1) WHERE id = $1")
        .bind(t.program).execute(&pool).await.unwrap();

    let sale = json!({"customer": "c1", "amount_cents": 10_000, "code": "REPLAY"});
    let ev = inbound(
        &pool,
        &t,
        "generic",
        "order-1",
        sale,
        true,
        Some("simulated failure"),
    )
    .await;
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversions WHERE program_id = $1")
            .bind(t.program)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(count().await, 0);

    let uri = format!("/api/v1/webhooks/inbound/{ev}/replay");
    let (st, body) = api.call(Method::POST, &uri, &t.admin, None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body["error"].is_null() && body["processed_at"].is_null());
    // A second replay while queued is a conflict.
    let (st, _) = api.call(Method::POST, &uri, &t.admin, None).await;
    assert_eq!(st, StatusCode::CONFLICT);

    api::worker::process_events(&pool).await.unwrap();
    assert_eq!(count().await, 1);
    let (error, done): (Option<String>, bool) =
        sqlx::query_as("SELECT error, processed_at IS NOT NULL FROM inbound_events WHERE id = $1")
            .bind(ev)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(error.is_none() && done);

    // Replaying a processed sale again does not double-count it.
    let (st, _) = api.call(Method::POST, &uri, &t.admin, None).await;
    assert_eq!(st, StatusCode::OK);
    api::worker::process_events(&pool).await.unwrap();
    assert_eq!(count().await, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn replay_inbound_other_workspace_is_404(pool: PgPool) {
    let api = Api::new(&pool, "http://127.0.0.1:1");
    let (a, b) = (tenant(&pool).await, tenant(&pool).await);
    let ev = inbound(&pool, &b, "generic", "x", json!({}), true, Some("e")).await;
    let (st, _) = api
        .call(
            Method::POST,
            &format!("/api/v1/webhooks/inbound/{ev}/replay"),
            &a.admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let err: Option<String> = sqlx::query_scalar("SELECT error FROM inbound_events WHERE id = $1")
        .bind(ev)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(err.as_deref(), Some("e"));
}
