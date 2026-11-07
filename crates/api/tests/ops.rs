use std::sync::{Arc, Mutex};

use api::auth::{Principal, issue_access};
use api::dispatch::{MAX_ATTEMPTS, deliver_due, dispatch_outbox, http_client};
use api::mail::Mailer;
use api::{AppState, app};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::routing::post;
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use hmac::{Hmac, Mac};
use lettre::transport::stub::AsyncStubTransport;
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-secret";

struct Api {
    app: Router,
}

impl Api {
    fn new(pool: &PgPool) -> Self {
        // The receivers below listen on 127.0.0.1, which the SSRF guard refuses by default.
        api::ssrf::allow_private_targets();
        let state = AppState {
            pool: pool.clone(),
            jwt_secret: SECRET.into(),
            vault: vault(),
            secure_cookies: true,
            limiter: Default::default(),
        };
        Self { app: app(state) }
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method.clone())
            .uri(uri)
            .header(HOST, "bf.test");
        if method != Method::GET {
            b = b.header(ORIGIN, "https://bf.test");
        }
        if let Some(c) = cookie {
            b = b.header(COOKIE, c);
        }
        let body = match body {
            Some(v) => {
                b = b.header(CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self
            .app
            .clone()
            .oneshot(b.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn get(&self, uri: &str, c: &str) -> (StatusCode, Value) {
        self.call(Method::GET, uri, Some(c), None).await
    }
    async fn post(&self, uri: &str, c: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, uri, Some(c), Some(body)).await
    }
    async fn post_empty(&self, uri: &str, c: &str) -> (StatusCode, Value) {
        self.call(Method::POST, uri, Some(c), None).await
    }
    async fn patch(&self, uri: &str, c: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::PATCH, uri, Some(c), Some(body)).await
    }
    async fn put(&self, uri: &str, c: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::PUT, uri, Some(c), Some(body)).await
    }
    async fn delete(&self, uri: &str, c: &str) -> StatusCode {
        self.call(Method::DELETE, uri, Some(c), None).await.0
    }
}

struct Tenant {
    ws: Uuid,
    slug: String,
    admin: String,
    program: Uuid,
}

/// A user row plus the `bf_at` cookie that authenticates it.
async fn user(pool: &PgPool, ws: Uuid, role: UserRole) -> (Uuid, String, String) {
    let u = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: format!("{}@x.io", Uuid::now_v7()),
        password_hash: "unused".into(),
        role,
    };
    create_user(pool, &u).await.unwrap();
    let token = issue_access(SECRET, &Principal::from(&u)).unwrap();
    (u.id, format!("bf_at={token}"), u.email)
}

/// Workspace with an admin and one program.
async fn tenant(pool: &PgPool, slug: &str) -> Tenant {
    let ws = Uuid::now_v7();
    create_workspace(pool, ws, slug, slug).await.unwrap();
    let (_, admin, _) = user(pool, ws, UserRole::Admin).await;
    let program = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
         VALUES ($1, $2, 'p', 'usd', $3, 's')",
    )
    .bind(program)
    .bind(ws)
    .bind(program.to_string())
    .execute(pool)
    .await
    .unwrap();
    Tenant {
        ws,
        slug: slug.into(),
        admin,
        program,
    }
}

/// Active affiliate of the tenant's program; returns (user id, cookie, email, affiliate id).
async fn affiliate(pool: &PgPool, t: &Tenant) -> (Uuid, String, String, Uuid) {
    let (user_id, cookie, email) = user(pool, t.ws, UserRole::Affiliate).await;
    let id = Uuid::now_v7();
    bf_db::affiliates::insert_affiliate(pool, id, t.ws, user_id, None)
        .await
        .unwrap();
    (user_id, cookie, email, id)
}

async fn outbox(pool: &PgPool, topic: &str, payload: Value) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO outbox (topic, payload) VALUES ($1, $2::text::jsonb) RETURNING id",
    )
    .bind(topic)
    .bind(payload.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn endpoint(api: &Api, t: &Tenant, url: &str, events: Value) -> Value {
    let (st, e) = api
        .post(
            "/api/v1/webhooks/endpoints",
            &t.admin,
            json!({ "url": url, "events": events }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{e}");
    e
}

// ---- local webhook receiver

type Seen = Arc<Mutex<Vec<(HeaderMap, String)>>>;

#[derive(Clone)]
struct Receiver {
    seen: Seen,
    /// Statuses to answer with, in order; 200 once exhausted.
    script: Arc<Mutex<Vec<u16>>>,
}

async fn receive(State(r): State<Receiver>, headers: HeaderMap, body: String) -> StatusCode {
    r.seen.lock().unwrap().push((headers, body));
    let next = {
        let mut script = r.script.lock().unwrap();
        (!script.is_empty()).then(|| script.remove(0))
    };
    StatusCode::from_u16(next.unwrap_or(200)).unwrap()
}

/// Serve on an ephemeral port; returns (url, requests seen).
async fn receiver(script: &[u16]) -> (String, Seen) {
    let seen = Seen::default();
    let r = Receiver {
        seen: seen.clone(),
        script: Arc::new(Mutex::new(script.to_vec())),
    };
    let app = Router::new().route("/hook", post(receive)).with_state(r);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

/// Check `X-Bf-Signature: t=<ts>,v1=<hex>` against an independently computed HMAC.
fn assert_signed(headers: &HeaderMap, body: &str, secret: &str) {
    let sig = headers["x-bf-signature"].to_str().unwrap();
    let (t, v1) = sig.split_once(",v1=").unwrap();
    let ts: i64 = t.strip_prefix("t=").unwrap().parse().unwrap();
    assert!((chrono::Utc::now().timestamp() - ts).abs() < 60);
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("{ts}.{body}").as_bytes());
    let want: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(v1, want);
}

type DeliveryRow = (i32, Option<i32>, bool);

/// `(attempt, status_code, pending)` of the only delivery.
async fn delivery(pool: &PgPool) -> DeliveryRow {
    sqlx::query_as(
        "SELECT attempt, status_code, next_attempt_at IS NOT NULL FROM webhook_deliveries",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn make_due(pool: &PgPool) {
    sqlx::query(
        "UPDATE webhook_deliveries SET next_attempt_at = now() WHERE next_attempt_at IS NOT NULL",
    )
    .execute(pool)
    .await
    .unwrap();
}

fn conversion_payload(t: &Tenant, affiliate: Uuid) -> Value {
    json!({
        "conversion_id": Uuid::now_v7(),
        "program_id": t.program,
        "affiliate_id": affiliate,
        "commission_cents": 1234,
        "currency": "usd",
        "status": "approved",
    })
}

// ---- webhooks

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_webhook_signed_retried_then_delivered(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    let (url, seen) = receiver(&[500]).await;
    let e = endpoint(&api, &t, &url, json!([])).await;
    let secret = e["secret"].as_str().unwrap();
    let payload = conversion_payload(&t, Uuid::now_v7());
    let id = outbox(&pool, "conversion.approved", payload.clone()).await;
    let mailer = Mailer::Log;
    let http = http_client();

    assert_eq!(dispatch_outbox(&pool, &mailer).await.unwrap(), 1);
    assert_eq!(
        dispatch_outbox(&pool, &mailer).await.unwrap(),
        0,
        "row is done"
    );
    assert_eq!(delivery(&pool).await, (0, None, true));

    // first attempt: receiver answers 500 -> recorded, retry scheduled in the future
    assert_eq!(deliver_due(&pool, &http, &vault()).await.unwrap(), 1);
    assert_eq!(delivery(&pool).await, (1, Some(500), true));
    assert_eq!(
        deliver_due(&pool, &http, &vault()).await.unwrap(),
        0,
        "backoff not elapsed"
    );
    let wait: i64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM next_attempt_at - now())::bigint FROM webhook_deliveries",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        (25..=30).contains(&wait),
        "first backoff is 30s, got {wait}"
    );

    // backoff elapsed: second attempt succeeds
    make_due(&pool).await;
    assert_eq!(deliver_due(&pool, &http, &vault()).await.unwrap(), 1);
    assert_eq!(delivery(&pool).await, (2, Some(200), false));
    assert_eq!(
        deliver_due(&pool, &http, &vault()).await.unwrap(),
        0,
        "delivered once"
    );

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (headers, body) in seen.iter() {
        assert_signed(headers, body, secret);
        assert_eq!(headers["x-bf-event"], "conversion.approved");
        let b: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            b,
            json!({ "id": id, "topic": "conversion.approved", "data": payload })
        );
    }
    assert_eq!(seen[0].1, seen[1].1, "a retry resends the same body");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_webhook_targets_only_active_matching_endpoints(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    let other = tenant(&pool, "other").await;
    let (url, seen) = receiver(&[]).await;
    let inactive = endpoint(&api, &t, &url, json!([])).await;
    let (st, _) = api
        .patch(
            &format!(
                "/api/v1/webhooks/endpoints/{}",
                inactive["id"].as_str().unwrap()
            ),
            &t.admin,
            json!({ "active": false }),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    endpoint(&api, &t, &url, json!(["payout.paid"])).await; // other topic
    endpoint(&api, &other, &url, json!([])).await; // other workspace
    outbox(
        &pool,
        "conversion.approved",
        conversion_payload(&t, Uuid::now_v7()),
    )
    .await;

    assert_eq!(dispatch_outbox(&pool, &Mailer::Log).await.unwrap(), 1);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        deliver_due(&pool, &http_client(), &vault()).await.unwrap(),
        0
    );
    assert!(seen.lock().unwrap().is_empty());

    // an endpoint switched off after queueing gets nothing either
    let live = endpoint(&api, &t, &url, json!(["conversion.approved"])).await;
    outbox(
        &pool,
        "conversion.approved",
        conversion_payload(&t, Uuid::now_v7()),
    )
    .await;
    dispatch_outbox(&pool, &Mailer::Log).await.unwrap();
    let uri = format!(
        "/api/v1/webhooks/endpoints/{}",
        live["id"].as_str().unwrap()
    );
    api.patch(&uri, &t.admin, json!({ "active": false })).await;
    assert_eq!(
        deliver_due(&pool, &http_client(), &vault()).await.unwrap(),
        0
    );
    assert!(seen.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_webhook_dead_after_max_attempts_then_redeliver(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    let (url, seen) = receiver(&vec![503; MAX_ATTEMPTS as usize]).await;
    endpoint(&api, &t, &url, json!([])).await;
    outbox(
        &pool,
        "conversion.created",
        conversion_payload(&t, Uuid::now_v7()),
    )
    .await;
    dispatch_outbox(&pool, &Mailer::Log).await.unwrap();
    let http = http_client();
    let delivery_id: Uuid = sqlx::query_scalar("SELECT id FROM webhook_deliveries")
        .fetch_one(&pool)
        .await
        .unwrap();
    let redeliver = format!("/api/v1/webhooks/deliveries/{delivery_id}/redeliver");

    for attempt in 1..=MAX_ATTEMPTS {
        assert_eq!(deliver_due(&pool, &http, &vault()).await.unwrap(), 1);
        let pending = attempt < MAX_ATTEMPTS;
        assert_eq!(delivery(&pool).await, (attempt, Some(503), pending));
        if pending {
            let (st, _) = api.post_empty(&redeliver, &t.admin).await;
            assert_eq!(st, StatusCode::CONFLICT, "still being retried");
            make_due(&pool).await;
        }
    }
    assert_eq!(seen.lock().unwrap().len(), MAX_ATTEMPTS as usize);
    assert_eq!(
        deliver_due(&pool, &http, &vault()).await.unwrap(),
        0,
        "dead"
    );

    let (st, d) = api.post_empty(&redeliver, &t.admin).await;
    assert_eq!(st, StatusCode::OK, "{d}");
    assert_eq!(d["attempt"], 0);
    assert_eq!(deliver_due(&pool, &http, &vault()).await.unwrap(), 1);
    assert_eq!(delivery(&pool).await, (1, Some(200), false));

    let (_, list) = api.get("/api/v1/webhooks/deliveries", &t.admin).await;
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["topic"], "conversion.created");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_webhook_endpoint_crud_scoping_and_guards(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    let other = tenant(&pool, "other").await;
    let (_, aff_cookie, _, _) = affiliate(&pool, &t).await;
    let base = "/api/v1/webhooks/endpoints";

    for bad in ["ftp://x.io/h", "not a url", "/relative", ""] {
        let (st, _) = api.post(base, &t.admin, json!({ "url": bad })).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (st, _) = api
        .post(
            base,
            &t.admin,
            json!({ "url": "https://x.io/h", "events": [""] }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let e = endpoint(&api, &t, "https://x.io/h", json!(["conversion.created"])).await;
    let uri = format!("{base}/{}", e["id"].as_str().unwrap());
    assert!(e["secret"].as_str().unwrap().starts_with("whsec_"));
    assert_eq!(e["active"], true);

    let (st, got) = api.get(&uri, &t.admin).await;
    assert_eq!(
        (st, &got["url"]),
        (StatusCode::OK, &json!("https://x.io/h"))
    );
    let (st, upd) = api
        .patch(
            &uri,
            &t.admin,
            json!({ "url": "https://y.io/h", "events": [] }),
        )
        .await;
    assert_eq!(
        (st, &upd["url"], &upd["events"]),
        (StatusCode::OK, &json!("https://y.io/h"), &json!([]))
    );
    assert_eq!(upd["secret"], "", "update never returns the secret");

    // guards and tenancy
    assert_eq!(
        api.call(Method::GET, base, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(api.get(base, &aff_cookie).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        api.post(base, &aff_cookie, json!({ "url": "https://x.io/h" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(api.get(&uri, &other.admin).await.0, StatusCode::NOT_FOUND);
    assert_eq!(api.delete(&uri, &other.admin).await, StatusCode::NOT_FOUND);
    let (_, list) = api.get(base, &other.admin).await;
    assert_eq!(list["total"], 0);

    assert_eq!(api.delete(&uri, &t.admin).await, StatusCode::NO_CONTENT);
    assert_eq!(api.get(&uri, &t.admin).await.0, StatusCode::NOT_FOUND);

    // audited, without the secret
    let (_, log) = api
        .get("/api/v1/audit-log?entity=webhook_endpoint", &t.admin)
        .await;
    let actions: Vec<_> = log["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        [
            "webhook_endpoint.delete",
            "webhook_endpoint.update",
            "webhook_endpoint.create"
        ]
    );
    assert!(!log.to_string().contains("whsec_"));
}

// ---- email, templates, notifications

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_conversion_approved_sends_email_and_notification(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    let (user_id, cookie, email, aff) = affiliate(&pool, &t).await;
    let (_, other_cookie, _, _) = affiliate(&pool, &t).await;
    let stub = AsyncStubTransport::new_ok();
    let mailer = Mailer::stub(stub.clone());

    outbox(&pool, "conversion.created", conversion_payload(&t, aff)).await; // no mail for this topic
    outbox(&pool, "conversion.approved", conversion_payload(&t, aff)).await;
    assert_eq!(dispatch_outbox(&pool, &mailer).await.unwrap(), 2);

    let sent = stub.messages().await;
    assert_eq!(sent.len(), 1);
    let (envelope, raw) = &sent[0];
    assert_eq!(envelope.to()[0].to_string(), email);
    assert!(
        raw.contains("Subject: Commission approved: 12.34 usd"),
        "{raw}"
    );
    assert!(
        raw.contains("Your commission of 12.34 usd was approved."),
        "{raw}"
    );

    // own notifications only
    let (st, list) = api.get("/api/v1/notifications", &cookie).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list["total"], 1);
    let n = &list["items"][0];
    assert_eq!(n["kind"], "conversion.approved");
    assert_eq!(n["body"], "Commission approved: 12.34 usd");
    assert_eq!(n["read_at"], Value::Null);
    let (_, theirs) = api.get("/api/v1/notifications", &other_cookie).await;
    assert_eq!(theirs["total"], 0);

    let uri = format!("/api/v1/notifications/{}/read", n["id"].as_str().unwrap());
    assert_eq!(
        api.post_empty(&uri, &other_cookie).await.0,
        StatusCode::NOT_FOUND
    );
    let (st, read) = api.post_empty(&uri, &cookie).await;
    assert_eq!(st, StatusCode::OK);
    assert_ne!(read["read_at"], Value::Null);
    let (_, again) = api.post_empty(&uri, &cookie).await;
    assert_eq!(
        again["read_at"], read["read_at"],
        "marking twice keeps the first time"
    );
    let (_, unread) = api.get("/api/v1/notifications?unread=true", &cookie).await;
    assert_eq!(unread["total"], 0);
    assert_eq!(
        api.call(Method::GET, "/api/v1/notifications", None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let _ = user_id;

    // a workspace template overrides the built-in text
    let (st, _) = api
        .post(
            "/api/v1/email-templates",
            &t.admin,
            json!({ "key": "conversion_approved", "subject": "Paid {{ commission }}!", "body": "Hi {{nope}}{{currency}}" }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    outbox(&pool, "conversion.approved", conversion_payload(&t, aff)).await;
    dispatch_outbox(&pool, &mailer).await.unwrap();
    let sent = stub.messages().await;
    assert_eq!(sent.len(), 2);
    assert!(sent[1].1.contains("Subject: Paid 12.34!"), "{}", sent[1].1);
    assert!(sent[1].1.ends_with("Hi usd"), "{}", sent[1].1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_email_template_crud_and_guards(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    let other = tenant(&pool, "other").await;
    let (_, aff_cookie, _, _) = affiliate(&pool, &t).await;
    let base = "/api/v1/email-templates";
    let new = json!({ "key": "payout_paid", "subject": "S {{amount}}", "body": "B" });

    assert_eq!(
        api.post(base, &aff_cookie, new.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    for bad in [
        json!({ "key": "nope", "subject": "S", "body": "B" }),
        json!({ "key": "payout_paid", "subject": "", "body": "B" }),
        json!({ "key": "payout_paid", "subject": "a\nBcc: x@y.io", "body": "B" }),
    ] {
        assert_eq!(
            api.post(base, &t.admin, bad).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (st, tpl) = api.post(base, &t.admin, new.clone()).await;
    assert_eq!(st, StatusCode::CREATED, "{tpl}");
    assert_eq!(
        api.post(base, &t.admin, new.clone()).await.0,
        StatusCode::CONFLICT
    );
    // same key in another workspace is fine
    assert_eq!(
        api.post(base, &other.admin, new).await.0,
        StatusCode::CREATED
    );

    let uri = format!("{base}/{}", tpl["id"].as_str().unwrap());
    let (st, upd) = api
        .patch(&uri, &t.admin, json!({ "body": "New body" }))
        .await;
    assert_eq!(
        (st, &upd["body"], &upd["subject"]),
        (StatusCode::OK, &json!("New body"), &json!("S {{amount}}"))
    );
    assert_eq!(api.get(&uri, &other.admin).await.0, StatusCode::NOT_FOUND);
    let (_, list) = api.get(base, &t.admin).await;
    assert_eq!(list["total"], 1);
    assert_eq!(api.delete(&uri, &other.admin).await, StatusCode::NOT_FOUND);
    assert_eq!(api.delete(&uri, &t.admin).await, StatusCode::NO_CONTENT);
    assert_eq!(api.get(&uri, &t.admin).await.0, StatusCode::NOT_FOUND);
}

// ---- contact requests

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_contact_flow(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    let other = tenant(&pool, "other").await;
    let (_, aff_cookie, _, _) = affiliate(&pool, &t).await;
    let msg = |ws: &str| json!({ "workspace": ws, "name": "Ann", "email": "ann@x.io", "message": "hello" });
    let base = "/api/v1/contact-requests";

    // public create: no cookie
    let (st, c) = api.call(Method::POST, base, None, Some(msg(&t.slug))).await;
    assert_eq!(st, StatusCode::CREATED, "{c}");
    assert_eq!(
        (&c["status"], &c["email"]),
        (&json!("new"), &json!("ann@x.io"))
    );
    let with_program = json!({ "workspace": t.slug, "program_id": t.program, "name": "Bo", "email": "bo@x.io", "message": "m" });
    assert_eq!(
        api.call(Method::POST, base, None, Some(with_program))
            .await
            .0,
        StatusCode::CREATED
    );

    for (bad, want) in [
        (
            json!({ "workspace": t.slug, "name": "", "email": "a@x.io", "message": "m" }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "workspace": t.slug, "name": "A", "email": "nope", "message": "m" }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "workspace": t.slug, "name": "A", "email": "a@x.io", "message": "x".repeat(5001) }),
            StatusCode::BAD_REQUEST,
        ),
        (msg("missing"), StatusCode::NOT_FOUND),
        // program of another workspace
        (
            json!({ "workspace": t.slug, "program_id": other.program, "name": "A", "email": "a@x.io", "message": "m" }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            api.call(Method::POST, base, None, Some(bad.clone()))
                .await
                .0,
            want,
            "{bad}"
        );
    }

    // admin side
    assert_eq!(api.get(base, &aff_cookie).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        api.call(Method::GET, base, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, list) = api.get(base, &t.admin).await;
    assert_eq!(list["total"], 2);
    let (_, none) = api.get(base, &other.admin).await;
    assert_eq!(none["total"], 0);

    let uri = format!("{base}/{}", c["id"].as_str().unwrap());
    assert_eq!(
        api.patch(&uri, &other.admin, json!({ "status": "read" }))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        api.patch(&uri, &aff_cookie, json!({ "status": "read" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        api.patch(&uri, &t.admin, json!({ "status": "bogus" }))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (st, upd) = api
        .patch(&uri, &t.admin, json!({ "status": "closed" }))
        .await;
    assert_eq!((st, &upd["status"]), (StatusCode::OK, &json!("closed")));
    let (_, new_only) = api.get(&format!("{base}?status=new"), &t.admin).await;
    assert_eq!(new_only["total"], 1);

    let (_, log) = api
        .get("/api/v1/audit-log?entity=contact_request", &t.admin)
        .await;
    let actions: Vec<_> = log["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        [
            "contact_request.update",
            "contact_request.create",
            "contact_request.create"
        ]
    );
}

// ---- settings and audit log

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_settings_and_audit_log(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    let other = tenant(&pool, "other").await;
    let (_, aff_cookie, _, _) = affiliate(&pool, &t).await;

    assert_eq!(
        api.get("/api/v1/settings", &aff_cookie).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        api.put("/api/v1/settings", &aff_cookie, json!({})).await.0,
        StatusCode::FORBIDDEN
    );
    let (st, s) = api.get("/api/v1/settings", &t.admin).await;
    assert_eq!((st, s), (StatusCode::OK, json!({})));
    for bad in [json!([1]), json!("x"), json!({ "k": "x".repeat(17_000) })] {
        assert_eq!(
            api.put("/api/v1/settings", &t.admin, bad).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let new = json!({ "brand": "Acme", "support_email": "s@acme.io" });
    let (st, put) = api.put("/api/v1/settings", &t.admin, new.clone()).await;
    assert_eq!((st, &put), (StatusCode::OK, &new));
    assert_eq!(api.get("/api/v1/settings", &t.admin).await.1, new);
    assert_eq!(
        api.get("/api/v1/settings", &other.admin).await.1,
        json!({}),
        "per workspace"
    );

    // audit: the settings write is there with keys only; pagination and filters work
    for i in 0..3 {
        let body = json!({ "name": format!("a{i}"), "currency": "usd" });
        assert_eq!(
            api.post("/api/v1/programs", &t.admin, body).await.0,
            StatusCode::CREATED
        );
    }
    let (st, log) = api.get("/api/v1/audit-log?limit=2", &t.admin).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(log["total"], 4);
    assert_eq!(log["limit"], 2);
    assert_eq!(log["items"].as_array().unwrap().len(), 2);
    let (_, page2) = api.get("/api/v1/audit-log?limit=2&page=2", &t.admin).await;
    assert_eq!(page2["items"].as_array().unwrap().len(), 2);
    assert_eq!(page2["items"][1]["action"], "settings.update");
    assert_eq!(
        page2["items"][1]["diff"],
        json!({ "keys": ["brand", "support_email"] })
    );
    assert!(!page2.to_string().contains("Acme"));
    let (_, progs) = api
        .get("/api/v1/audit-log?action=program.create", &t.admin)
        .await;
    assert_eq!(progs["total"], 3);
    let (_, ents) = api
        .get("/api/v1/audit-log?entity=workspace", &t.admin)
        .await;
    assert_eq!(ents["total"], 1);

    assert_eq!(
        api.get("/api/v1/audit-log", &aff_cookie).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        api.call(Method::GET, "/api/v1/audit-log", None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, theirs) = api.get("/api/v1/audit-log", &other.admin).await;
    assert_eq!(theirs["total"], 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_slow_endpoint_holds_no_lock_while_in_flight(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    // An endpoint that accepts the connection and never answers.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock);
        }
    });
    endpoint(&api, &t, &url, json!([])).await;
    outbox(
        &pool,
        "conversion.approved",
        conversion_payload(&t, Uuid::now_v7()),
    )
    .await;
    dispatch_outbox(&pool, &Mailer::Log).await.unwrap();

    let p = pool.clone();
    let sending = tokio::spawn(async move { deliver_due(&p, &http_client(), &vault()).await });
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;

    // The request is still in flight, yet the row is leased (not due again for a while) and
    // not locked: nothing holds a transaction open across the HTTP call.
    let lease: i64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM next_attempt_at - now())::bigint FROM webhook_deliveries",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(lease > 30, "leased past the next tick, got {lease}s");
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM webhook_deliveries FOR UPDATE NOWAIT")
        .fetch_one(&mut *tx)
        .await
        .expect("row is not locked while the request is in flight");
    drop(tx);
    assert_eq!(
        deliver_due(&pool, &http_client(), &vault()).await.unwrap(),
        0,
        "a leased delivery is not sent twice"
    );
    sending.abort();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ops_webhook_endpoints_are_capped_per_workspace(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "acme").await;
    for i in 0..20 {
        endpoint(&api, &t, &format!("http://127.0.0.1:9/h{i}"), json!([])).await;
    }
    let (st, body) = api
        .post(
            "/api/v1/webhooks/endpoints",
            &t.admin,
            json!({ "url": "http://127.0.0.1:9/extra", "events": [] }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
}

/// A real key ring, so the delivery tests exercise sealed endpoint secrets.
fn vault() -> std::sync::Arc<bf_core::secrets::Vault> {
    use bf_core::secrets::{KeyRing, Vault};
    let ring = KeyRing::from_pairs("t1", &[("t1", [9u8; 32])]).unwrap();
    std::sync::Arc::new(Vault::new(ring))
}
