use std::time::Duration;

use api::auth::{Principal, issue_access};
use api::{AppState, app};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{Method, Request, StatusCode};
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use bf_db::tracker::{CONFIG_CHANNEL, load_codes};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-secret";

struct Api {
    app: Router,
}

impl Api {
    fn new(pool: &PgPool) -> Self {
        let state = AppState {
            pool: pool.clone(),
            jwt_secret: SECRET.into(),
            vault: std::sync::Arc::new(bf_core::secrets::Vault::plaintext()),
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
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
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
    async fn delete(&self, uri: &str, c: &str) -> StatusCode {
        self.call(Method::DELETE, uri, Some(c), None).await.0
    }
}

struct Tenant {
    ws: Uuid,
    admin: String,
}

/// A user row plus the `bf_at` cookie that authenticates it.
async fn user(pool: &PgPool, ws: Option<Uuid>, role: UserRole) -> (Uuid, String) {
    let u = User {
        id: Uuid::now_v7(),
        workspace_id: ws,
        email: format!("{}@x.io", Uuid::now_v7()),
        password_hash: "unused".into(),
        role,
    };
    create_user(pool, &u).await.unwrap();
    let token = issue_access(SECRET, &Principal::from(&u)).unwrap();
    (u.id, format!("bf_at={token}"))
}

async fn tenant(pool: &PgPool, slug: &str) -> Tenant {
    let ws = Uuid::now_v7();
    create_workspace(pool, ws, slug, slug).await.unwrap();
    let (_, admin) = user(pool, Some(ws), UserRole::Admin).await;
    Tenant { ws, admin }
}

/// Active affiliate with a login cookie; returns (affiliate id, cookie).
async fn affiliate(pool: &PgPool, ws: Uuid) -> (Uuid, String) {
    let (user_id, cookie) = user(pool, Some(ws), UserRole::Affiliate).await;
    let id = Uuid::now_v7();
    bf_db::affiliates::insert_affiliate(pool, id, ws, user_id, None)
        .await
        .unwrap();
    bf_db::affiliates::set_affiliate_status(pool, ws, id, &["pending"], "active")
        .await
        .unwrap();
    (id, cookie)
}

async fn program(api: &Api, t: &Tenant, name: &str) -> String {
    let body = json!({ "name": name, "currency": "usd" });
    let (st, p) = api.post("/api/v1/programs", &t.admin, body).await;
    assert_eq!(st, StatusCode::CREATED, "{p}");
    p["id"].as_str().unwrap().to_string()
}

async fn site(api: &Api, t: &Tenant, program: &str, domain: &str) {
    let uri = format!("/api/v1/programs/{program}/sites");
    let (st, b) = api.post(&uri, &t.admin, json!({ "domain": domain })).await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
}

async fn enroll(api: &Api, t: &Tenant, program: &str, affiliate: Uuid) {
    let uri = format!("/api/v1/programs/{program}/enrollments");
    let (st, b) = api
        .post(&uri, &t.admin, json!({ "affiliate_id": affiliate }))
        .await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
}

async fn listener(pool: &PgPool) -> PgListener {
    let mut l = PgListener::connect_with(pool).await.unwrap();
    l.listen(CONFIG_CHANNEL).await.unwrap();
    l
}

async fn notified(l: &mut PgListener, wait_ms: u64) -> bool {
    tokio::time::timeout(Duration::from_millis(wait_ms), l.recv())
        .await
        .is_ok_and(|n| n.is_ok())
}

async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_role_guards(pool: PgPool) {
    let api = Api::new(&pool);
    let a = tenant(&pool, "a").await;
    let (_, super_admin) = user(&pool, Some(a.ws), UserRole::SuperAdmin).await;
    let (aff_id, aff) = affiliate(&pool, a.ws).await;
    let p = program(&api, &a, "P").await;

    // no cookie
    let (st, _) = api.call(Method::GET, "/api/v1/programs", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // affiliate cannot use admin routes
    let (st, _) = api
        .post(
            "/api/v1/programs",
            &aff,
            json!({ "name": "X", "currency": "USD" }),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    for uri in [
        "/api/v1/affiliates".to_string(),
        "/api/v1/referral-codes".to_string(),
        "/api/v1/members".to_string(),
        format!("/api/v1/programs/{p}/sites"),
        format!("/api/v1/programs/{p}/plans"),
    ] {
        assert_eq!(api.get(&uri, &aff).await.0, StatusCode::FORBIDDEN, "{uri}");
    }
    let (st, _) = api
        .post_empty(&format!("/api/v1/affiliates/{aff_id}/approve"), &aff)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // admin cannot use super_admin or affiliate-only routes
    assert_eq!(
        api.get("/api/v1/workspaces", &a.admin).await.0,
        StatusCode::FORBIDDEN
    );
    let (st, _) = api
        .post("/api/v1/workspaces", &a.admin, json!({ "name": "N" }))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(
        api.get("/api/v1/me/affiliate", &a.admin).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        api.get("/api/v1/me/referral-codes", &a.admin).await.0,
        StatusCode::FORBIDDEN
    );

    // super_admin passes both
    assert_eq!(
        api.get("/api/v1/workspaces", &super_admin).await.0,
        StatusCode::OK
    );
    assert_eq!(
        api.get("/api/v1/programs", &super_admin).await.0,
        StatusCode::OK
    );

    // an affiliate sees programs only after enrollment, never the secrets
    let (st, list) = api.get("/api/v1/programs", &aff).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list["total"], 0);
    assert_eq!(
        api.get(&format!("/api/v1/programs/{p}"), &aff).await.0,
        StatusCode::NOT_FOUND
    );
    enroll(&api, &a, &p, aff_id).await;
    let (st, seen) = api.get(&format!("/api/v1/programs/{p}"), &aff).await;
    assert_eq!(st, StatusCode::OK);
    assert!(seen.get("webhook_secret").is_none() && seen.get("stripe_secret").is_none());
    let (_, listed) = api.get("/api/v1/programs", &aff).await;
    assert!(listed["items"][0].get("webhook_secret").is_none());
    let (_, full) = api.get(&format!("/api/v1/programs/{p}"), &a.admin).await;
    assert!(
        full.get("webhook_secret").is_none(),
        "admin reads never return the secret; it is shown once on create"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_workspace_isolation(pool: PgPool) {
    let api = Api::new(&pool);
    let (a, b) = (tenant(&pool, "a").await, tenant(&pool, "b").await);
    let pa = program(&api, &a, "A prog").await;
    let pb = program(&api, &b, "B prog").await;
    site(&api, &a, &pa, "a.example.com").await;
    let (aff_a, _) = affiliate(&pool, a.ws).await;
    let (aff_b, _) = affiliate(&pool, b.ws).await;
    enroll(&api, &a, &pa, aff_a).await;
    let (st, code) = api
        .post(
            "/api/v1/referral-codes",
            &a.admin,
            json!({ "affiliate_id": aff_a, "program_id": pa, "code": "acode" }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{code}");
    let (st, plan) = api
        .post(
            &format!("/api/v1/programs/{pa}/plans"),
            &a.admin,
            json!({ "kind": "flat", "amount_cents": 500, "duration": "lifetime" }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let plan_id = plan["id"].as_str().unwrap();

    // B sees none of A's data
    let (_, list) = api.get("/api/v1/programs", &b.admin).await;
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["id"], pb);
    assert_eq!(api.get("/api/v1/affiliates", &b.admin).await.1["total"], 1);
    assert_eq!(
        api.get("/api/v1/referral-codes", &b.admin).await.1["total"],
        0
    );
    assert_eq!(api.get("/api/v1/members", &b.admin).await.1["total"], 1);

    let gone = StatusCode::NOT_FOUND;
    assert_eq!(
        api.get(&format!("/api/v1/programs/{pa}"), &b.admin).await.0,
        gone
    );
    let (st, _) = api
        .patch(
            &format!("/api/v1/programs/{pa}"),
            &b.admin,
            json!({ "name": "hax" }),
        )
        .await;
    assert_eq!(st, gone);
    assert_eq!(
        api.post_empty(&format!("/api/v1/programs/{pa}/secrets/rotate"), &b.admin)
            .await
            .0,
        gone
    );
    assert_eq!(
        api.delete(&format!("/api/v1/programs/{pa}"), &b.admin)
            .await,
        gone
    );
    for tail in ["sites", "conversion-types", "plans", "enrollments"] {
        let uri = format!("/api/v1/programs/{pa}/{tail}");
        assert_eq!(api.get(&uri, &b.admin).await.0, gone, "{uri}");
    }
    let (st, _) = api
        .post(
            &format!("/api/v1/programs/{pa}/sites"),
            &b.admin,
            json!({ "domain": "evil.com" }),
        )
        .await;
    assert_eq!(st, gone);
    let (st, _) = api
        .post(
            &format!("/api/v1/programs/{pa}/plans"),
            &b.admin,
            json!({ "kind": "flat", "amount_cents": 1, "duration": "lifetime" }),
        )
        .await;
    assert_eq!(st, gone);
    assert_eq!(
        api.delete(&format!("/api/v1/programs/{pa}/plans/{plan_id}"), &b.admin)
            .await,
        gone
    );
    assert_eq!(
        api.delete(
            &format!("/api/v1/programs/{pa}/sites/a.example.com"),
            &b.admin
        )
        .await,
        gone
    );

    // affiliates and enrollments across the boundary
    assert_eq!(
        api.get(&format!("/api/v1/affiliates/{aff_a}"), &b.admin)
            .await
            .0,
        gone
    );
    assert_eq!(
        api.post_empty(&format!("/api/v1/affiliates/{aff_a}/suspend"), &b.admin)
            .await
            .0,
        gone
    );
    let (st, _) = api
        .patch(
            &format!("/api/v1/affiliates/{aff_a}"),
            &b.admin,
            json!({ "payout_min_cents": 1 }),
        )
        .await;
    assert_eq!(st, gone);
    let (st, _) = api
        .post(
            &format!("/api/v1/programs/{pb}/enrollments"),
            &b.admin,
            json!({ "affiliate_id": aff_a }),
        )
        .await;
    assert_eq!(st, gone);
    let (st, _) = api
        .post(
            &format!("/api/v1/programs/{pa}/enrollments"),
            &b.admin,
            json!({ "affiliate_id": aff_b }),
        )
        .await;
    assert_eq!(st, gone);
    assert_eq!(
        api.delete(
            &format!("/api/v1/programs/{pa}/enrollments/{aff_a}"),
            &b.admin
        )
        .await,
        gone
    );

    // codes across the boundary
    let (st, _) = api
        .post(
            "/api/v1/referral-codes",
            &b.admin,
            json!({ "affiliate_id": aff_a, "program_id": pa }),
        )
        .await;
    assert_eq!(st, gone);
    let (st, _) = api
        .post(
            "/api/v1/referral-codes",
            &b.admin,
            json!({ "affiliate_id": aff_b, "program_id": pa }),
        )
        .await;
    assert_eq!(st, gone);
    assert_eq!(
        api.delete("/api/v1/referral-codes/acode", &b.admin).await,
        gone
    );

    // nothing of A's changed
    let (_, still) = api.get(&format!("/api/v1/programs/{pa}"), &a.admin).await;
    assert_eq!(still["name"], "A prog");
    assert_eq!(
        api.get("/api/v1/referral-codes", &a.admin).await.1["total"],
        1
    );
    assert_eq!(
        api.get(&format!("/api/v1/programs/{pa}/enrollments"), &a.admin)
            .await
            .1["total"],
        1
    );

    // an explicit foreign workspace is refused for admins
    let uri = format!("/api/v1/members?workspace_id={}", a.ws);
    assert_eq!(api.get(&uri, &b.admin).await.0, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_plan_validation(pool: PgPool) {
    let api = Api::new(&pool);
    let (a, b) = (tenant(&pool, "a").await, tenant(&pool, "b").await);
    let p = program(&api, &a, "P").await;
    let other = program(&api, &b, "Other").await;
    let uri = format!("/api/v1/programs/{p}/plans");

    let bad = [
        json!({ "kind": "percent", "rate_bps": 10001, "duration": "lifetime" }),
        json!({ "kind": "percent", "duration": "lifetime" }),
        json!({ "kind": "percent", "rate_bps": 1000, "amount_cents": 5, "duration": "lifetime" }),
        json!({ "kind": "percent", "rate_bps": -1, "duration": "lifetime" }),
        json!({ "kind": "flat", "duration": "lifetime" }),
        json!({ "kind": "flat", "amount_cents": -1, "duration": "lifetime" }),
        json!({ "kind": "flat", "amount_cents": 100, "rate_bps": 5, "duration": "lifetime" }),
        json!({ "kind": "percent", "rate_bps": 1000, "duration": "x_months" }),
        json!({ "kind": "percent", "rate_bps": 1000, "duration": "x_months", "months": 0 }),
        json!({ "kind": "percent", "rate_bps": 1000, "duration": "lifetime", "months": 3 }),
        json!({ "kind": "percent", "rate_bps": 1000, "duration": "one_month", "months": 1 }),
        json!({ "kind": "bogus", "rate_bps": 1000, "duration": "lifetime" }),
        json!({ "kind": "percent", "rate_bps": 1000, "duration": "forever" }),
        json!({ "kind": "percent", "rate_bps": 1000, "duration": "lifetime", "conversion_type_id": Uuid::now_v7() }),
    ];
    for body in bad {
        let (st, res) = api.post(&uri, &a.admin, body.clone()).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body} -> {res}");
        assert_eq!(res["error"]["code"], "bad_request");
    }
    // a conversion type of another program is not accepted
    let (_, ct) = api
        .post(
            &format!("/api/v1/programs/{other}/conversion-types"),
            &b.admin,
            json!({ "key": "k", "name": "K" }),
        )
        .await;
    let body = json!({ "kind": "flat", "amount_cents": 1, "duration": "lifetime", "conversion_type_id": ct["id"] });
    assert_eq!(
        api.post(&uri, &a.admin, body).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(api.get(&uri, &a.admin).await.1["total"], 0);

    // valid plans round-trip
    let (st, pct) = api
        .post(
            &uri,
            &a.admin,
            json!({ "kind": "percent", "rate_bps": 2500, "duration": "x_months", "months": 6 }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{pct}");
    assert_eq!(
        (pct["kind"].as_str(), pct["rate_bps"].as_i64()),
        (Some("percent"), Some(2500))
    );
    assert_eq!(
        (pct["duration"].as_str(), pct["months"].as_i64()),
        (Some("x_months"), Some(6))
    );
    assert!(pct["amount_cents"].is_null());
    let (st, flat) = api
        .post(
            &uri,
            &a.admin,
            json!({ "kind": "flat", "amount_cents": 1500, "duration": "one_month" }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(flat["amount_cents"], 1500);
    assert!(flat["rate_bps"].is_null() && flat["months"].is_null());
    assert_eq!(api.get(&uri, &a.admin).await.1["total"], 2);

    // a plan can become the default, a foreign plan cannot, and a default can't be deleted
    let pid = pct["id"].as_str().unwrap();
    let prog = format!("/api/v1/programs/{p}");
    let (st, upd) = api
        .patch(&prog, &a.admin, json!({ "default_plan_id": pid }))
        .await;
    assert_eq!(st, StatusCode::OK, "{upd}");
    assert_eq!(upd["default_plan_id"], pid);
    let (_, bplan) = api
        .post(
            &format!("/api/v1/programs/{other}/plans"),
            &b.admin,
            json!({ "kind": "flat", "amount_cents": 1, "duration": "lifetime" }),
        )
        .await;
    let (st, _) = api
        .patch(&prog, &a.admin, json!({ "default_plan_id": bplan["id"] }))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(
        api.delete(&format!("{uri}/{pid}"), &a.admin).await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api.delete(&format!("{uri}/{}", flat["id"].as_str().unwrap()), &a.admin)
            .await,
        StatusCode::NO_CONTENT
    );

    // program fields are validated too
    for body in [
        json!({ "name": "P", "currency": "US" }),
        json!({ "name": "P", "currency": "US1" }),
        json!({ "name": " ", "currency": "USD" }),
        json!({ "name": "P", "currency": "USD", "cookie_days": 0 }),
        json!({ "name": "P", "currency": "USD", "hold_days": -1 }),
        json!({ "name": "P", "currency": "USD", "attribution_model": "random" }),
    ] {
        assert_eq!(
            api.post("/api/v1/programs", &a.admin, body).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_notify_on_code_create(pool: PgPool) {
    let api = Api::new(&pool);
    let a = tenant(&pool, "a").await;
    let mut l = listener(&pool).await;

    let p = program(&api, &a, "P").await;
    assert!(notified(&mut l, 2000).await, "program create");
    site(&api, &a, &p, "shop.example.com").await;
    let (aff, aff_cookie) = affiliate(&pool, a.ws).await;
    assert!(
        !notified(&mut l, 200).await,
        "sites and affiliates don't touch the snapshot"
    );

    enroll(&api, &a, &p, aff).await;
    assert!(notified(&mut l, 2000).await, "enroll");

    // a rejected create rolls back: no row, no notification
    let (st, _) = api
        .post("/api/v1/referral-codes", &a.admin, json!({ "affiliate_id": aff, "program_id": p, "code": "bad", "landing_url": "https://evil.com/x" }))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(!notified(&mut l, 200).await, "rejected create");

    let body = json!({ "affiliate_id": aff, "program_id": p, "code": "promo1" });
    let (st, code) = api.post("/api/v1/referral-codes", &a.admin, body).await;
    assert_eq!(st, StatusCode::CREATED, "{code}");
    assert_eq!(code["landing_url"], "https://shop.example.com");
    assert!(notified(&mut l, 2000).await, "code create");
    // the tracker's snapshot query sees it
    let rows = load_codes(&pool).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].code.as_str(), rows[0].landing_url.as_str()),
        ("promo1", "https://shop.example.com")
    );

    // own code, generated
    let (st, own) = api
        .post(
            "/api/v1/me/referral-codes",
            &aff_cookie,
            json!({ "program_id": p }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{own}");
    assert!(notified(&mut l, 2000).await, "me code create");

    assert_eq!(
        api.delete("/api/v1/referral-codes/promo1", &a.admin).await,
        StatusCode::NO_CONTENT
    );
    assert!(notified(&mut l, 2000).await, "code deactivate");
    assert_eq!(load_codes(&pool).await.unwrap().len(), 1);

    let (st, rotated) = api
        .post_empty(&format!("/api/v1/programs/{p}/secrets/rotate"), &a.admin)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(notified(&mut l, 2000).await, "rotate");
    let (_, again) = api
        .post_empty(&format!("/api/v1/programs/{p}/secrets/rotate"), &a.admin)
        .await;
    assert_ne!(rotated["webhook_secret"], again["webhook_secret"]);
    assert!(notified(&mut l, 2000).await, "second rotate");

    let (st, _) = api
        .patch(
            &format!("/api/v1/programs/{p}"),
            &a.admin,
            json!({ "stripe_secret": "sk_1" }),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(notified(&mut l, 2000).await, "provider secret");

    // unenrolling drops the affiliate's remaining codes
    assert_eq!(
        api.delete(&format!("/api/v1/programs/{p}/enrollments/{aff}"), &a.admin)
            .await,
        StatusCode::NO_CONTENT
    );
    assert!(notified(&mut l, 2000).await, "unenroll");
    assert!(load_codes(&pool).await.unwrap().is_empty());

    assert_eq!(
        api.delete(&format!("/api/v1/programs/{p}"), &a.admin).await,
        StatusCode::NO_CONTENT
    );
    assert!(notified(&mut l, 2000).await, "program delete");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_pagination(pool: PgPool) {
    let api = Api::new(&pool);
    let a = tenant(&pool, "a").await;
    for i in 0..5 {
        program(&api, &a, &format!("P{i}")).await;
    }
    let mut seen = vec![];
    for (page, want) in [(1, 2), (2, 2), (3, 1), (4, 0)] {
        let (st, res) = api
            .get(&format!("/api/v1/programs?page={page}&limit=2"), &a.admin)
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(res["total"], 5);
        assert_eq!(
            (res["page"].as_u64(), res["limit"].as_u64()),
            (Some(page), Some(2))
        );
        let items = res["items"].as_array().unwrap();
        assert_eq!(items.len(), want, "page {page}");
        seen.extend(items.iter().map(|p| p["id"].as_str().unwrap().to_string()));
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!((seen.len(), unique.len()), (5, 5));

    // defaults and clamping
    let (_, d) = api.get("/api/v1/programs", &a.admin).await;
    assert_eq!(
        (
            d["page"].as_u64(),
            d["limit"].as_u64(),
            d["items"].as_array().unwrap().len()
        ),
        (Some(1), Some(50), 5)
    );
    let (_, c) = api
        .get("/api/v1/programs?page=0&limit=100000", &a.admin)
        .await;
    assert_eq!(
        (c["page"].as_u64(), c["limit"].as_u64()),
        (Some(1), Some(200))
    );
    let (_, z) = api.get("/api/v1/programs?limit=0", &a.admin).await;
    assert_eq!(z["limit"], 1);
    assert_eq!(
        api.get("/api/v1/programs?page=abc", &a.admin).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_affiliate_lifecycle(pool: PgPool) {
    let api = Api::new(&pool);
    let (a, b) = (tenant(&pool, "a").await, tenant(&pool, "b").await);
    let p = program(&api, &a, "P").await;
    site(&api, &a, &p, "example.com").await;

    let reg = |ws: &str, email: &str, pw: &str| json!({ "workspace": ws, "email": email, "password": pw, "payout_email": "pay@x.io" });
    let public = |body| {
        api.call(
            Method::POST,
            "/api/v1/affiliates/register",
            None,
            Some(body),
        )
    };
    let (st, aff) = public(reg("a", "new@x.io", "longenough")).await;
    assert_eq!(st, StatusCode::CREATED, "{aff}");
    assert_eq!(
        (aff["status"].as_str(), aff["email"].as_str()),
        (Some("pending"), Some("new@x.io"))
    );
    assert_eq!(aff["payout_email"], "pay@x.io");
    let id = aff["id"].as_str().unwrap().to_string();
    assert_eq!(
        public(reg("a", "new@x.io", "longenough")).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        public(reg("nope", "z@x.io", "longenough")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        public(reg("a", "z@x.io", "short")).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        public(reg("a", "not-an-email", "longenough")).await.0,
        StatusCode::BAD_REQUEST
    );
    // register needs the Origin header like every mutation
    let no_origin = Request::builder()
        .method("POST")
        .uri("/api/v1/affiliates/register")
        .header(HOST, "bf.test")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(reg("a", "q@x.io", "longenough").to_string()))
        .unwrap();
    assert_eq!(
        api.app.clone().oneshot(no_origin).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(audit_count(&pool, "affiliate.register").await, 1);

    let (_, pending) = api.get("/api/v1/affiliates?status=pending", &a.admin).await;
    assert_eq!(pending["total"], 1);
    assert_eq!(
        api.get("/api/v1/affiliates?status=active", &a.admin)
            .await
            .1["total"],
        0
    );
    assert_eq!(
        api.get("/api/v1/affiliates?status=pending", &b.admin)
            .await
            .1["total"],
        0
    );

    // pending affiliates can't get codes; transitions are checked
    enroll_raw(&api, &a, &p, &id).await;
    let code_body = json!({ "affiliate_id": id, "program_id": p });
    assert_eq!(
        api.post("/api/v1/referral-codes", &a.admin, code_body.clone())
            .await
            .0,
        StatusCode::CONFLICT
    );
    let act = |verb: &str| format!("/api/v1/affiliates/{id}/{verb}");
    assert_eq!(
        api.post_empty(&act("reactivate"), &a.admin).await.0,
        StatusCode::CONFLICT
    );
    let (st, ok) = api.post_empty(&act("approve"), &a.admin).await;
    assert_eq!(
        (st, ok["status"].as_str()),
        (StatusCode::OK, Some("active"))
    );
    assert_eq!(
        api.post_empty(&act("approve"), &a.admin).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api.post("/api/v1/referral-codes", &a.admin, code_body.clone())
            .await
            .0,
        StatusCode::CREATED
    );
    let (_, sus) = api.post_empty(&act("suspend"), &a.admin).await;
    assert_eq!(sus["status"], "suspended");
    assert_eq!(
        api.post_empty(&act("suspend"), &a.admin).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api.post("/api/v1/referral-codes", &a.admin, code_body.clone())
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (_, back) = api.post_empty(&act("reactivate"), &a.admin).await;
    assert_eq!(back["status"], "active");
    assert_eq!(audit_count(&pool, "affiliate.active").await, 2);
    assert_eq!(audit_count(&pool, "affiliate.suspended").await, 1);

    // payout settings: admin and the affiliate themself
    let (st, upd) = api
        .patch(
            &format!("/api/v1/affiliates/{id}"),
            &a.admin,
            json!({ "payout_email": "p2@x.io", "payout_min_cents": 2500 }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{upd}");
    assert_eq!(
        (
            upd["payout_email"].as_str(),
            upd["payout_min_cents"].as_i64()
        ),
        (Some("p2@x.io"), Some(2500))
    );
    let user_id: Uuid = upd["user_id"].as_str().unwrap().parse().unwrap();
    let me = Principal {
        user_id,
        role: UserRole::Affiliate,
        workspace_id: Some(a.ws),
    };
    let cookie = format!("bf_at={}", issue_access(SECRET, &me).unwrap());
    let (st, mine) = api
        .patch(
            "/api/v1/me/affiliate",
            &cookie,
            json!({ "payout_min_cents": 100 }),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        (
            mine["payout_min_cents"].as_i64(),
            mine["payout_email"].as_str()
        ),
        (Some(100), Some("p2@x.io"))
    );
    assert_eq!(api.get("/api/v1/me/affiliate", &cookie).await.1["id"], id);
    for bad in [
        json!({ "payout_min_cents": -1 }),
        json!({ "payout_email": "nope" }),
    ] {
        assert_eq!(
            api.patch("/api/v1/me/affiliate", &cookie, bad).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}

async fn enroll_raw(api: &Api, t: &Tenant, program: &str, affiliate: &str) {
    let uri = format!("/api/v1/programs/{program}/enrollments");
    let (st, b) = api
        .post(&uri, &t.admin, json!({ "affiliate_id": affiliate }))
        .await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_referral_codes(pool: PgPool) {
    let api = Api::new(&pool);
    let a = tenant(&pool, "a").await;
    let p = program(&api, &a, "P").await;
    let (aff, mine) = affiliate(&pool, a.ws).await;
    let (other, theirs) = affiliate(&pool, a.ws).await;
    // no sites -> no landing page; not enrolled -> conflict
    let (st, _) = api
        .post(
            "/api/v1/me/referral-codes",
            &mine,
            json!({ "program_id": p }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    site(&api, &a, &p, "Example.com").await;
    let (st, _) = api
        .post(
            "/api/v1/me/referral-codes",
            &mine,
            json!({ "program_id": p }),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
    enroll(&api, &a, &p, aff).await;
    enroll(&api, &a, &p, other).await;

    // landing_url must stay on the program's sites
    for url in [
        "https://evil.com",
        "https://example.com.evil.com",
        "https://evil.com/?u=example.com",
        "https://user@example.com",
        "https://evil.com\\@example.com",
        "javascript:alert(1)",
        "//example.com",
        "https://",
    ] {
        let body = json!({ "program_id": p, "landing_url": url });
        let (st, res) = api.post("/api/v1/me/referral-codes", &mine, body).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{url} -> {res}");
    }
    for code in ["ab", "has space", "semi;colon", &"x".repeat(33)] {
        let body = json!({ "program_id": p, "code": code });
        assert_eq!(
            api.post("/api/v1/me/referral-codes", &mine, body).await.0,
            StatusCode::BAD_REQUEST,
            "{code}"
        );
    }
    let body = json!({ "program_id": p, "code": "Mine-1", "landing_url": "https://shop.example.com/pricing?x=1" });
    let (st, c) = api.post("/api/v1/me/referral-codes", &mine, body).await;
    assert_eq!(st, StatusCode::CREATED, "{c}");
    assert_eq!(
        (c["affiliate_id"].as_str(), c["code"].as_str()),
        (Some(aff.to_string().as_str()), Some("Mine-1"))
    );

    // codes are unique across affiliates
    let dup = json!({ "program_id": p, "code": "Mine-1" });
    assert_eq!(
        api.post("/api/v1/me/referral-codes", &theirs, dup).await.0,
        StatusCode::CONFLICT
    );
    let dup = json!({ "affiliate_id": other, "program_id": p, "code": "Mine-1" });
    assert_eq!(
        api.post("/api/v1/referral-codes", &a.admin, dup).await.0,
        StatusCode::CONFLICT
    );

    // visibility and ownership
    let (_, list) = api.get("/api/v1/me/referral-codes", &mine).await;
    assert_eq!(list["total"], 1);
    assert_eq!(
        api.get("/api/v1/me/referral-codes", &theirs).await.1["total"],
        0
    );
    assert_eq!(
        api.delete("/api/v1/me/referral-codes/Mine-1", &theirs)
            .await,
        StatusCode::NOT_FOUND
    );
    let (_, filtered) = api
        .get(
            &format!("/api/v1/referral-codes?affiliate_id={other}"),
            &a.admin,
        )
        .await;
    assert_eq!(filtered["total"], 0);
    let (_, by_prog) = api
        .get(&format!("/api/v1/referral-codes?program_id={p}"), &a.admin)
        .await;
    assert_eq!(by_prog["total"], 1);

    assert_eq!(
        api.delete("/api/v1/me/referral-codes/Mine-1", &mine).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        api.delete("/api/v1/me/referral-codes/Mine-1", &mine).await,
        StatusCode::NOT_FOUND
    );
    // freed codes can be taken again
    let again = json!({ "program_id": p, "code": "Mine-1" });
    assert_eq!(
        api.post("/api/v1/me/referral-codes", &theirs, again)
            .await
            .0,
        StatusCode::CREATED
    );

    assert_eq!(audit_count(&pool, "code.create").await, 2);
    assert_eq!(audit_count(&pool, "code.deactivate").await, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_workspaces_members_and_audit(pool: PgPool) {
    let api = Api::new(&pool);
    let home = tenant(&pool, "home").await;
    let (_, root) = user(&pool, Some(home.ws), UserRole::SuperAdmin).await;

    let (st, ws) = api
        .post("/api/v1/workspaces", &root, json!({ "name": "Acme Inc" }))
        .await;
    assert_eq!(st, StatusCode::CREATED, "{ws}");
    assert_eq!(ws["slug"], "acme-inc");
    let new_ws = ws["id"].as_str().unwrap().to_string();
    assert_eq!(
        api.post(
            "/api/v1/workspaces",
            &root,
            json!({ "name": "Other", "slug": "acme-inc" })
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api.post("/api/v1/workspaces", &root, json!({ "name": " " }))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, all) = api.get("/api/v1/workspaces?limit=1&page=2", &root).await;
    assert_eq!(
        (
            all["total"].as_i64(),
            all["items"].as_array().unwrap().len()
        ),
        (Some(2), 1)
    );

    // super_admin seeds the first admin of the new workspace; that admin can log in as a member
    let invite = json!({ "email": "boss@acme.io", "password": "longenough" });
    let uri = format!("/api/v1/members?workspace_id={new_ws}");
    let (st, m) = api.post(&uri, &root, invite.clone()).await;
    assert_eq!(st, StatusCode::CREATED, "{m}");
    assert_eq!(m["role"], "admin");
    assert!(m.get("password").is_none() && m.get("password_hash").is_none());
    assert_eq!(api.post(&uri, &root, invite).await.0, StatusCode::CONFLICT);
    let (_, members) = api.get(&uri, &root).await;
    assert_eq!(
        (
            members["total"].as_i64(),
            members["items"][0]["email"].as_str()
        ),
        (Some(1), Some("boss@acme.io"))
    );
    let short = json!({ "email": "x@acme.io", "password": "short" });
    assert_eq!(
        api.post(&uri, &root, short).await.0,
        StatusCode::BAD_REQUEST
    );

    // admins invite into their own workspace only
    let own = json!({ "email": "peer@home.io", "password": "longenough" });
    assert_eq!(
        api.post("/api/v1/members", &home.admin, own).await.0,
        StatusCode::CREATED
    );
    assert_eq!(api.get("/api/v1/members", &home.admin).await.1["total"], 3);
    let foreign = json!({ "email": "sneak@home.io", "password": "longenough" });
    assert_eq!(
        api.post(&uri, &home.admin, foreign).await.0,
        StatusCode::FORBIDDEN
    );

    // every mutation left an audit row in the right workspace, without secrets
    assert_eq!(audit_count(&pool, "workspace.create").await, 1);
    let in_new: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'member.invite' AND workspace_id = $1",
    )
    .bind(Uuid::parse_str(&new_ws).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(in_new, 1);

    let p = program(&api, &home, "P").await;
    let prog = format!("/api/v1/programs/{p}");
    api.patch(
        &prog,
        &home.admin,
        json!({ "stripe_secret": "sk_live_topsecret", "cookie_days": 10 }),
    )
    .await;
    api.post_empty(&format!("{prog}/secrets/rotate"), &home.admin)
        .await;
    for (action, n) in [
        ("program.create", 1),
        ("program.update", 1),
        ("program.rotate_secret", 1),
    ] {
        assert_eq!(audit_count(&pool, action).await, n, "{action}");
    }
    let leaked: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE diff::text LIKE '%topsecret%' OR diff::text LIKE '%whsec_%'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(leaked, 0);

    // sites, conversion types: normalization, uniqueness, deletion
    site(&api, &home, &p, " Shop.Example.COM ").await;
    let sites_uri = format!("{prog}/sites");
    assert_eq!(
        api.get(&sites_uri, &home.admin).await.1["items"][0]["domain"],
        "shop.example.com"
    );
    assert_eq!(
        api.post(
            &sites_uri,
            &home.admin,
            json!({ "domain": "shop.example.com" })
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for bad in ["http://x.com", "a b.com", "x.com/path", "", "-x.com"] {
        assert_eq!(
            api.post(&sites_uri, &home.admin, json!({ "domain": bad }))
                .await
                .0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    assert_eq!(
        api.delete(&format!("{sites_uri}/shop.example.com"), &home.admin)
            .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        api.delete(&format!("{sites_uri}/shop.example.com"), &home.admin)
            .await,
        StatusCode::NOT_FOUND
    );

    let ct_uri = format!("{prog}/conversion-types");
    let (st, ct) = api
        .post(
            &ct_uri,
            &home.admin,
            json!({ "key": "signup", "name": "Signup" }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(
        api.post(
            &ct_uri,
            &home.admin,
            json!({ "key": "signup", "name": "Again" })
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let plan = json!({ "kind": "flat", "amount_cents": 100, "duration": "lifetime", "conversion_type_id": ct["id"] });
    let (_, pl) = api.post(&format!("{prog}/plans"), &home.admin, plan).await;
    let ct_id = ct["id"].as_str().unwrap();
    assert_eq!(
        api.delete(&format!("{ct_uri}/{ct_id}"), &home.admin).await,
        StatusCode::CONFLICT
    );
    api.delete(
        &format!("{prog}/plans/{}", pl["id"].as_str().unwrap()),
        &home.admin,
    )
    .await;
    assert_eq!(
        api.delete(&format!("{ct_uri}/{ct_id}"), &home.admin).await,
        StatusCode::NO_CONTENT
    );

    // enrollment with a plan override; changing it is an upsert
    let (aff, _) = affiliate(&pool, home.ws).await;
    let (_, plan) = api
        .post(
            &format!("{prog}/plans"),
            &home.admin,
            json!({ "kind": "percent", "rate_bps": 3000, "duration": "lifetime" }),
        )
        .await;
    let enroll_uri = format!("{prog}/enrollments");
    let (st, e) = api
        .post(
            &enroll_uri,
            &home.admin,
            json!({ "affiliate_id": aff, "plan_override_id": plan["id"] }),
        )
        .await;
    assert_eq!(
        (st, &e["plan_override_id"]),
        (StatusCode::CREATED, &plan["id"])
    );
    let (_, e2) = api
        .post(&enroll_uri, &home.admin, json!({ "affiliate_id": aff }))
        .await;
    assert!(e2["plan_override_id"].is_null());
    assert_eq!(api.get(&enroll_uri, &home.admin).await.1["total"], 1);
    let bogus = json!({ "affiliate_id": aff, "plan_override_id": Uuid::now_v7() });
    assert_eq!(
        api.post(&enroll_uri, &home.admin, bogus).await.0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_code_landing_url_and_count_are_capped(pool: PgPool) {
    let api = Api::new(&pool);
    let a = tenant(&pool, "a").await;
    let pa = program(&api, &a, "A prog").await;
    site(&api, &a, &pa, "a.example.com").await;
    let (aff, _) = affiliate(&pool, a.ws).await;
    enroll(&api, &a, &pa, aff).await;

    let long = format!("https://a.example.com/{}", "p".repeat(2100));
    let (st, body) = api
        .post(
            "/api/v1/referral-codes",
            &a.admin,
            json!({ "affiliate_id": aff, "program_id": pa, "landing_url": long }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

    for i in 0..100 {
        let (st, body) = api
            .post(
                "/api/v1/referral-codes",
                &a.admin,
                json!({ "affiliate_id": aff, "program_id": pa, "code": format!("code{i:03}") }),
            )
            .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
    }
    let (st, body) = api
        .post(
            "/api/v1/referral-codes",
            &a.admin,
            json!({ "affiliate_id": aff, "program_id": pa, "code": "one-too-many" }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_currency_and_day_bounds(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "a").await;
    for body in [
        json!({ "name": "Yen", "currency": "JPY" }),
        json!({ "name": "Dinar", "currency": "KWD" }),
        json!({ "name": "Long hold", "currency": "USD", "hold_days": 366 }),
        json!({ "name": "Forever", "currency": "USD", "hold_days": 2_000_000_000 }),
        json!({ "name": "Long cookie", "currency": "USD", "cookie_days": 366 }),
    ] {
        let (st, b) = api.post("/api/v1/programs", &t.admin, body.clone()).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}: {b}");
    }
    let (st, b) = api
        .post(
            "/api/v1/programs",
            &t.admin,
            json!({ "name": "Ok", "currency": "eur", "hold_days": 365, "cookie_days": 365 }),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    let p = b["id"].as_str().unwrap();
    let (st, _) = api
        .patch(
            &format!("/api/v1/programs/{p}"),
            &t.admin,
            json!({ "hold_days": 400 }),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn programs_payout_email_is_tri_state_and_frozen_when_inactive(pool: PgPool) {
    let api = Api::new(&pool);
    let t = tenant(&pool, "a").await;
    let (aff, _) = affiliate(&pool, t.ws).await;
    let uri = format!("/api/v1/affiliates/{aff}");

    let (st, a) = api
        .patch(&uri, &t.admin, json!({ "payout_email": "pay@x.io" }))
        .await;
    assert_eq!(st, StatusCode::OK, "{a}");
    assert_eq!(a["payout_email"], "pay@x.io");

    // Absent leaves it alone.
    let (_, a) = api
        .patch(&uri, &t.admin, json!({ "payout_min_cents": 100 }))
        .await;
    assert_eq!(a["payout_email"], "pay@x.io");
    assert_eq!(a["payout_min_cents"], 100);

    // An explicit null clears it.
    let (st, a) = api
        .patch(&uri, &t.admin, json!({ "payout_email": null }))
        .await;
    assert_eq!(st, StatusCode::OK, "{a}");
    assert_eq!(a["payout_email"], Value::Null);

    // A suspended affiliate cannot be pointed somewhere else.
    bf_db::affiliates::set_affiliate_status(&pool, t.ws, aff, &["active"], "suspended")
        .await
        .unwrap();
    let (st, b) = api
        .patch(&uri, &t.admin, json!({ "payout_email": "elsewhere@x.io" }))
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{b}");
    let (st, _) = api
        .patch(&uri, &t.admin, json!({ "payout_min_cents": 5 }))
        .await;
    assert_eq!(st, StatusCode::OK, "other settings still change");
}
