use api::auth::{Principal, issue_access};
use api::worker::process_events;
use api::{AppState, app};
use axum::body::Body;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use bf_db::auth::{User, UserRole};
use bf_db::tracker::{InboundEvent, insert_events};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-secret";

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

struct Api(axum::Router);

impl Api {
    fn new(pool: &PgPool) -> Self {
        Self(app(AppState {
            pool: pool.clone(),
            jwt_secret: SECRET.into(),
            secure_cookies: true,
        }))
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> Resp {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(HOST, "bf.test")
            .header(ORIGIN, "https://bf.test");
        if let Some(c) = cookie {
            b = b.header(COOKIE, c);
        }
        let body = body.map_or_else(Body::empty, |v| {
            b = std::mem::take(&mut b).header(CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        });
        let res = self.0.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        Resp {
            status,
            headers,
            body: String::from_utf8(bytes.to_vec()).unwrap(),
        }
    }

    async fn get(&self, uri: &str, cookie: &str) -> Resp {
        self.call(Method::GET, uri, Some(cookie), None).await
    }

    async fn post(&self, uri: &str, cookie: &str) -> Resp {
        self.call(Method::POST, uri, Some(cookie), None).await
    }

    async fn create_batch(&self, program: Uuid, cookie: &str) -> Resp {
        let body = json!({ "program_id": program });
        self.call(
            Method::POST,
            "/api/v1/payouts/batches",
            Some(cookie),
            Some(body),
        )
        .await
    }
}

/// A workspace with a manual-approval program (USD, 10% lifetime), an admin and two active
/// affiliates A (`a@x.io`) and B (`b@x.io`), each with a cookie.
struct World {
    program: Uuid,
    admin: String,
    a: (Uuid, String),
    b: (Uuid, String),
}

async fn user(pool: &PgPool, ws: Uuid, role: UserRole, email: &str) -> (Uuid, String) {
    let u = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: email.into(),
        password_hash: "unused".into(),
        role,
    };
    bf_db::auth::create_user(pool, &u).await.unwrap();
    let token = issue_access(SECRET, &Principal::from(&u)).unwrap();
    (u.id, format!("bf_at={token}"))
}

async fn affiliate(
    pool: &PgPool,
    ws: Uuid,
    program: Uuid,
    code: &str,
    email: &str,
) -> (Uuid, String) {
    let (user_id, cookie) = user(
        pool,
        ws,
        UserRole::Affiliate,
        &format!("login-{ws}-{email}"),
    )
    .await;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO affiliates (id, workspace_id, user_id, status, payout_email) \
         VALUES ($1, $2, $3, 'active', $4)",
    )
    .bind(id)
    .bind(ws)
    .bind(user_id)
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO enrollments (affiliate_id, program_id) VALUES ($1, $2)")
        .bind(id)
        .bind(program)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) \
         VALUES ($1, $2, $3, 'https://x.io/')",
    )
    .bind(code)
    .bind(id)
    .bind(program)
    .execute(pool)
    .await
    .unwrap();
    (id, cookie)
}

async fn world(pool: &PgPool) -> World {
    let (ws, program, plan) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    bf_db::auth::create_workspace(pool, ws, &ws.to_string(), &ws.to_string())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, auto_approve, hold_days, \
           public_key, webhook_secret) VALUES ($1, $2, 'p', 'USD', false, 0, $3, 's')",
    )
    .bind(program)
    .bind(ws)
    .bind(program.to_string())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commission_plans (id, program_id, kind, rate_bps, duration) \
         VALUES ($1, $2, 'percent', 1000, 'lifetime')",
    )
    .bind(plan)
    .bind(program)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE programs SET default_plan_id = $2 WHERE id = $1")
        .bind(program)
        .bind(plan)
        .execute(pool)
        .await
        .unwrap();
    let tag = ws.simple().to_string();
    World {
        program,
        admin: user(pool, ws, UserRole::Admin, &format!("admin-{tag}@x.io"))
            .await
            .1,
        a: affiliate(pool, ws, program, &format!("A{tag}"), "a@x.io").await,
        b: affiliate(pool, ws, program, &format!("B{tag}"), "b@x.io").await,
    }
}

/// A sale credited to an affiliate through the real ingest path (10% commission); returns the
/// conversion id. The order id doubles as customer and external id.
async fn sale(pool: &PgPool, w: &World, order: &str, cents: i64, aff: Uuid) -> Uuid {
    let code: String =
        sqlx::query_scalar("SELECT code FROM referral_codes WHERE affiliate_id = $1")
            .bind(aff)
            .fetch_one(pool)
            .await
            .unwrap();
    let ext = format!("{order}-{}", w.program);
    insert_events(
        pool,
        &[InboundEvent {
            id: Uuid::now_v7(),
            program_id: w.program,
            source: "generic".into(),
            external_id: ext.clone(),
            payload: json!({"customer": ext, "amount_cents": cents, "code": code,
                            "occurred_at": (Utc::now() - Duration::hours(1)).to_rfc3339()}),
            received_ms: Utc::now().timestamp_millis(),
        }],
    )
    .await
    .unwrap();
    process_events(pool).await.unwrap();
    sqlx::query_scalar("SELECT id FROM conversions WHERE external_id = $1 AND program_id = $2")
        .bind(ext)
        .bind(w.program)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Every txn balances to zero.
async fn assert_balanced(pool: &PgPool) {
    let bad: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT txn_id FROM ledger_entries GROUP BY txn_id \
         HAVING sum(amount_cents) <> 0) t",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(bad, 0);
}

async fn entry_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `(pending, payable)` of one affiliate as the balances route reports them.
async fn balance(api: &Api, cookie: &str, aff: Uuid) -> (i64, i64) {
    let r = api
        .get(
            &format!("/api/v1/ledger/balances?affiliate_id={aff}"),
            cookie,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let j = r.json();
    match j["items"].as_array().unwrap().as_slice() {
        [] => (0, 0),
        [b] => (
            b["pending_cents"].as_i64().unwrap(),
            b["payable_cents"].as_i64().unwrap(),
        ),
        more => panic!("one row per program expected, got {}", more.len()),
    }
}

fn uri(path: &str, id: Uuid, tail: &str) -> String {
    format!("/api/v1/{path}/{id}{tail}")
}

#[sqlx::test(migrations = "../db/migrations")]
async fn money_routes_role_guards(pool: PgPool) {
    let w = world(&pool).await;
    let conv = sale(&pool, &w, "o1", 10_000, w.a.0).await;
    let api = Api::new(&pool);
    let id = Uuid::now_v7();

    let admin_gets = [
        "/api/v1/customers".to_string(),
        uri("customers", id, ""),
        "/api/v1/conversions".into(),
        uri("conversions", conv, ""),
        "/api/v1/ledger/entries".into(),
        "/api/v1/payouts/batches".into(),
        uri("payouts/batches", id, ""),
        uri("payouts/batches", id, "/export.csv"),
    ];
    let admin_posts = [
        uri("conversions", conv, "/approve"),
        uri("conversions", conv, "/reject"),
        uri("conversions", conv, "/reverse"),
        uri("payouts/batches", id, "/approve"),
        uri("payouts/batches", id, "/mark-paid"),
        uri("payouts/batches", id, "/cancel"),
    ];
    for u in &admin_gets {
        assert_eq!(
            api.call(Method::GET, u, None, None).await.status,
            StatusCode::UNAUTHORIZED,
            "{u}"
        );
        assert_eq!(
            api.get(u, &w.a.1).await.status,
            StatusCode::FORBIDDEN,
            "{u}"
        );
    }
    for u in &admin_posts {
        assert_eq!(
            api.call(Method::POST, u, None, None).await.status,
            StatusCode::UNAUTHORIZED,
            "{u}"
        );
        assert_eq!(
            api.post(u, &w.a.1).await.status,
            StatusCode::FORBIDDEN,
            "{u}"
        );
    }
    assert_eq!(
        api.create_batch(w.program, &w.a.1).await.status,
        StatusCode::FORBIDDEN
    );
    // the affiliate's own routes are closed to admins
    for u in ["/api/v1/me/earnings", "/api/v1/me/payouts"] {
        assert_eq!(
            api.get(u, &w.admin).await.status,
            StatusCode::FORBIDDEN,
            "{u}"
        );
        assert_eq!(
            api.call(Method::GET, u, None, None).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    // nothing above changed the conversion
    let status: String = sqlx::query_scalar("SELECT status::text FROM conversions WHERE id = $1")
        .bind(conv)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "pending");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn money_routes_workspace_isolation(pool: PgPool) {
    let (w1, w2) = (world(&pool).await, world(&pool).await);
    let conv = sale(&pool, &w1, "o1", 10_000, w1.a.0).await;
    let api = Api::new(&pool);
    assert_eq!(
        api.post(&uri("conversions", conv, "/approve"), &w1.admin)
            .await
            .status,
        StatusCode::OK
    );
    let batch = api.create_batch(w1.program, &w1.admin).await.json()["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let customer: Uuid = sqlx::query_scalar("SELECT customer_id FROM conversions WHERE id = $1")
        .bind(conv)
        .fetch_one(&pool)
        .await
        .unwrap();

    // workspace 1 sees its data
    for u in [
        "customers",
        "conversions",
        "ledger/entries",
        "ledger/balances",
        "payouts/batches",
    ] {
        let j = api.get(&format!("/api/v1/{u}"), &w1.admin).await.json();
        assert!(j["total"].as_i64().unwrap() > 0, "{u}");
    }

    // workspace 2 sees none of it
    for u in [
        "customers",
        "conversions",
        "ledger/entries",
        "ledger/balances",
        "payouts/batches",
    ] {
        let j = api.get(&format!("/api/v1/{u}"), &w2.admin).await.json();
        assert_eq!(j["total"], 0, "{u}");
        assert_eq!(j["items"].as_array().unwrap().len(), 0, "{u}");
    }
    let foreign_filters = [
        format!("/api/v1/conversions?program_id={}", w1.program),
        format!("/api/v1/customers?program_id={}", w1.program),
        format!("/api/v1/ledger/entries?program_id={}", w1.program),
    ];
    for u in &foreign_filters {
        assert_eq!(api.get(u, &w2.admin).await.json()["total"], 0, "{u}");
    }
    assert_eq!(
        api.get(
            &format!("/api/v1/ledger/balances?program_id={}", w1.program),
            &w2.admin
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    for u in [
        uri("conversions", conv, ""),
        uri("customers", customer, ""),
        uri("payouts/batches", batch, ""),
        uri("payouts/batches", batch, "/export.csv"),
    ] {
        assert_eq!(
            api.get(&u, &w2.admin).await.status,
            StatusCode::NOT_FOUND,
            "{u}"
        );
    }
    for u in [
        uri("conversions", conv, "/approve"),
        uri("conversions", conv, "/reject"),
        uri("conversions", conv, "/reverse"),
        uri("payouts/batches", batch, "/approve"),
        uri("payouts/batches", batch, "/mark-paid"),
        uri("payouts/batches", batch, "/cancel"),
    ] {
        assert_eq!(
            api.post(&u, &w2.admin).await.status,
            StatusCode::NOT_FOUND,
            "{u}"
        );
    }
    assert_eq!(
        api.create_batch(w1.program, &w2.admin).await.status,
        StatusCode::NOT_FOUND
    );
    // and none of the attempts changed anything
    let (conv_status, batch_status): (String, String) = sqlx::query_as(
        "SELECT (SELECT status::text FROM conversions WHERE id = $1), \
                (SELECT status::text FROM payout_batches WHERE id = $2)",
    )
    .bind(conv)
    .bind(batch)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (conv_status.as_str(), batch_status.as_str()),
        ("approved", "draft")
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn money_routes_affiliate_sees_only_own(pool: PgPool) {
    let w = world(&pool).await;
    let o1 = sale(&pool, &w, "o1", 10_000, w.a.0).await;
    let o2 = sale(&pool, &w, "o2", 5_050, w.a.0).await;
    let o3 = sale(&pool, &w, "o3", 20_000, w.b.0).await;
    let api = Api::new(&pool);
    for c in [o1, o2, o3] {
        assert_eq!(
            api.post(&uri("conversions", c, "/approve"), &w.admin)
                .await
                .status,
            StatusCode::OK
        );
    }

    // earnings: own conversions only, no provider ids
    let ea = api.get("/api/v1/me/earnings", &w.a.1).await.json();
    assert_eq!(ea["total"], 2);
    let mut got: Vec<(i64, i64)> = ea["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["amount_cents"].as_i64().unwrap(),
                i["commission_cents"].as_i64().unwrap(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(got, [(5_050, 505), (10_000, 1_000)]);
    assert!(ea["items"][0].get("external_id").is_none());
    let eb = api
        .get("/api/v1/me/earnings?status=approved", &w.b.1)
        .await
        .json();
    assert_eq!(eb["total"], 1);
    assert_eq!(eb["items"][0]["commission_cents"], 2_000);
    let none = api
        .get("/api/v1/me/earnings?status=paid", &w.b.1)
        .await
        .json();
    assert_eq!(none["total"], 0);

    // balances: own row only; another affiliate's id is a 404
    let ba = api.get("/api/v1/ledger/balances", &w.a.1).await.json();
    assert_eq!(ba["total"], 1);
    assert_eq!(ba["items"][0]["affiliate_id"], w.a.0.to_string());
    assert_eq!(ba["items"][0]["payable_cents"], 1_505);
    assert_eq!(
        api.get(
            &format!("/api/v1/ledger/balances?affiliate_id={}", w.b.0),
            &w.a.1
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(balance(&api, &w.a.1, w.a.0).await, (0, 1_505));
    let all = api.get("/api/v1/ledger/balances", &w.admin).await.json();
    assert_eq!(all["total"], 2);
    assert_eq!(balance(&api, &w.admin, w.b.0).await, (0, 2_000));

    // payouts: drafts stay internal, then each affiliate sees only their own line
    let batch = api.create_batch(w.program, &w.admin).await.json()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        api.get("/api/v1/me/payouts", &w.a.1).await.json()["total"],
        0
    );
    let step = |s: &str| format!("/api/v1/payouts/batches/{batch}/{s}");
    assert_eq!(
        api.post(&step("approve"), &w.admin).await.status,
        StatusCode::OK
    );
    let pa = api.get("/api/v1/me/payouts", &w.a.1).await.json();
    assert_eq!(pa["total"], 1);
    assert_eq!(pa["items"][0]["amount_cents"], 1_505);
    assert_eq!(pa["items"][0]["status"], "approved");
    assert_eq!(pa["items"][0]["currency"], "USD");
    assert_eq!(
        api.post(&step("mark-paid"), &w.admin).await.status,
        StatusCode::OK
    );
    let pb = api.get("/api/v1/me/payouts", &w.b.1).await.json();
    assert_eq!(pb["total"], 1);
    assert_eq!(pb["items"][0]["amount_cents"], 2_000);
    assert_eq!(pb["items"][0]["status"], "paid");
    assert!(pb["items"][0]["paid_at"].is_string());
    assert_eq!(balance(&api, &w.a.1, w.a.0).await, (0, 0));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn money_routes_conversion_actions_keep_ledger_balanced(pool: PgPool) {
    let w = world(&pool).await;
    let o1 = sale(&pool, &w, "o1", 10_000, w.a.0).await;
    let o2 = sale(&pool, &w, "o2", 5_050, w.a.0).await;
    let o3 = sale(&pool, &w, "o3", 20_000, w.b.0).await;
    let api = Api::new(&pool);
    assert_eq!(balance(&api, &w.admin, w.a.0).await, (1_505, 0));

    // approve: pending -> payable
    let r = api
        .post(&uri("conversions", o1, "/approve"), &w.admin)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let j = r.json();
    assert_eq!(j["status"], "approved");
    assert!(j["approved_at"].is_string());
    assert_eq!(balance(&api, &w.admin, w.a.0).await, (505, 1_000));

    // transitions the state machine forbids: 409, no ledger change, no audit row
    let (entries, audits) = (entry_count(&pool).await, audit_count(&pool).await);
    for u in [
        uri("conversions", o1, "/approve"),
        uri("conversions", o1, "/reject"),
        uri("conversions", o2, "/reverse"),
    ] {
        assert_eq!(
            api.post(&u, &w.admin).await.status,
            StatusCode::CONFLICT,
            "{u}"
        );
    }
    assert_eq!(entry_count(&pool).await, entries);
    assert_eq!(audit_count(&pool).await, audits);

    // reject: pending -> expense
    let j = api
        .post(&uri("conversions", o2, "/reject"), &w.admin)
        .await
        .json();
    assert_eq!(j["status"], "rejected");
    assert!(j["reversed_at"].is_string());
    assert_eq!(balance(&api, &w.admin, w.a.0).await, (0, 1_000));

    // reverse: payable -> expense
    let j = api
        .post(&uri("conversions", o1, "/reverse"), &w.admin)
        .await
        .json();
    assert_eq!(j["status"], "reversed");
    assert_eq!(balance(&api, &w.admin, w.a.0).await, (0, 0));
    assert_eq!(
        api.post(&uri("conversions", o1, "/reverse"), &w.admin)
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(balance(&api, &w.admin, w.b.0).await, (2_000, 0));
    assert_balanced(&pool).await;

    // the whole history of A nets to zero in the books; every action left an audit row + outbox
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_log ORDER BY at, id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        actions,
        [
            "conversion.approve",
            "conversion.reject",
            "conversion.reverse"
        ]
    );
    let (approved, reversed): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE topic = 'conversion.approved'), \
                count(*) FILTER (WHERE topic = 'conversion.reversed') FROM outbox",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((approved, reversed), (1, 2));

    // filters and paging
    let day: String = sqlx::query_scalar(
        "SELECT to_char(occurred_at AT TIME ZONE 'UTC', 'YYYY-MM-DD') FROM conversions WHERE id = $1",
    )
    .bind(o3)
    .fetch_one(&pool)
    .await
    .unwrap();
    let total = |q: String| {
        let api = &api;
        let admin = &w.admin;
        async move {
            let r = api.get(&format!("/api/v1/conversions?{q}"), admin).await;
            assert_eq!(r.status, StatusCode::OK, "{q}: {}", r.body);
            r.json()
        }
    };
    assert_eq!(total(String::new()).await["total"], 3);
    assert_eq!(
        total("status=pending".into()).await["items"][0]["id"],
        o3.to_string()
    );
    assert_eq!(total("status=rejected".into()).await["total"], 1);
    assert_eq!(total(format!("affiliate_id={}", w.b.0)).await["total"], 1);
    assert_eq!(
        total(format!("affiliate_id={}&status=reversed", w.a.0)).await["total"],
        1
    );
    assert_eq!(total(format!("program_id={}", w.program)).await["total"], 3);
    assert_eq!(total(format!("from={day}&to={day}")).await["total"], 3);
    assert_eq!(total("from=2999-01-01".into()).await["total"], 0);
    assert_eq!(total("to=2000-01-01".into()).await["total"], 0);
    let page = total("limit=2&page=2".into()).await;
    assert_eq!(
        (
            page["total"].clone(),
            page["items"].as_array().unwrap().len()
        ),
        (json!(3), 1)
    );
    for bad in ["from=nope", "from=2026-02-02&to=2026-02-01", "status=bogus"] {
        let r = api
            .get(&format!("/api/v1/conversions?{bad}"), &w.admin)
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // customers
    let cs = api.get("/api/v1/customers", &w.admin).await.json();
    assert_eq!(cs["total"], 3);
    let by_b = api
        .get(
            &format!("/api/v1/customers?affiliate_id={}", w.b.0),
            &w.admin,
        )
        .await
        .json();
    assert_eq!(by_b["total"], 1);
    let id = by_b["items"][0]["id"].as_str().unwrap();
    let one = api
        .get(&format!("/api/v1/customers/{id}"), &w.admin)
        .await
        .json();
    assert_eq!(one["affiliate_id"], w.b.0.to_string());
    assert!(one["external_id"].as_str().unwrap().starts_with("o3-"));

    // ledger entries: by account, by txn
    let payable = api
        .get("/api/v1/ledger/entries?account=aff_payable", &w.admin)
        .await
        .json();
    assert_eq!(payable["total"], 2); // o1 approve credit + o1 reverse debit
    let expense = api
        .get(
            &format!(
                "/api/v1/ledger/entries?account=expense&affiliate_id={}",
                w.a.0
            ),
            &w.admin,
        )
        .await;
    assert_eq!(expense.json()["total"], 0); // expense legs carry no affiliate
    let txn = payable["items"][0]["txn_id"].as_str().unwrap();
    let legs = api
        .get(&format!("/api/v1/ledger/entries?txn_id={txn}"), &w.admin)
        .await
        .json();
    let amounts: Vec<i64> = legs["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["amount_cents"].as_i64().unwrap())
        .collect();
    assert_eq!(amounts.len(), 2);
    assert_eq!(amounts.iter().sum::<i64>(), 0);
    assert_eq!(
        api.get("/api/v1/ledger/entries?account=bogus", &w.admin)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

async fn audit_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn money_routes_payout_create_approve_paid_csv(pool: PgPool) {
    let w = world(&pool).await;
    let api = Api::new(&pool);
    // nothing approved yet: nobody is owed
    assert_eq!(
        api.create_batch(w.program, &w.admin).await.status,
        StatusCode::CONFLICT
    );
    let o1 = sale(&pool, &w, "o1", 10_000, w.a.0).await;
    let o2 = sale(&pool, &w, "o2", 5_050, w.a.0).await;
    let o3 = sale(&pool, &w, "o3", 20_000, w.b.0).await;
    for c in [o1, o2, o3] {
        assert_eq!(
            api.post(&uri("conversions", c, "/approve"), &w.admin)
                .await
                .status,
            StatusCode::OK
        );
    }

    // create -> cancel releases the conversions -> create again
    let r = api.create_batch(w.program, &w.admin).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let first = r.json()["id"].as_str().unwrap().to_owned();
    let cancel = api
        .post(&format!("/api/v1/payouts/batches/{first}/cancel"), &w.admin)
        .await;
    assert_eq!(cancel.json()["status"], "cancelled");
    assert_eq!(
        api.post(
            &format!("/api/v1/payouts/batches/{first}/approve"),
            &w.admin
        )
        .await
        .status,
        StatusCode::CONFLICT
    );

    let r = api.create_batch(w.program, &w.admin).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let created = r.json();
    let batch = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["status"], "draft");
    assert_eq!(created["currency"], "USD");
    assert_eq!(created["payout_count"], 2);
    assert_eq!(created["total_cents"], 3_505);
    let lines: Vec<(&str, i64)> = created["payouts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["email"].as_str().unwrap(),
                p["amount_cents"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], ("a@x.io", 1_505));
    assert_eq!(lines[1], ("b@x.io", 2_000));
    // a second batch right away would double-claim: nothing left to pay
    assert_eq!(
        api.create_batch(w.program, &w.admin).await.status,
        StatusCode::CONFLICT
    );

    // cannot pay a draft
    let step = |s: &str| format!("/api/v1/payouts/batches/{batch}/{s}");
    assert_eq!(
        api.post(&step("mark-paid"), &w.admin).await.status,
        StatusCode::CONFLICT
    );
    let before = entry_count(&pool).await;

    let approved = api.post(&step("approve"), &w.admin).await.json();
    assert_eq!(approved["status"], "approved");
    assert!(approved["approved_by"].is_string());
    assert_eq!(entry_count(&pool).await, before, "approve posts nothing");

    let paid = api.post(&step("mark-paid"), &w.admin).await.json();
    assert_eq!(paid["status"], "paid");
    assert!(paid["paid_at"].is_string());
    assert!(
        paid["payouts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["status"] == "paid" && p["ledger_txn_id"].is_string())
    );
    assert_eq!(
        api.post(&step("mark-paid"), &w.admin).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        entry_count(&pool).await,
        before + 4,
        "two payouts, two legs each"
    );
    assert_balanced(&pool).await;

    // books: payable -> cash
    assert_eq!(balance(&api, &w.admin, w.a.0).await, (0, 0));
    assert_eq!(balance(&api, &w.admin, w.b.0).await, (0, 0));
    let cash = api
        .get("/api/v1/ledger/entries?account=cash", &w.admin)
        .await
        .json();
    let mut cash_legs: Vec<i64> = cash["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["amount_cents"].as_i64().unwrap())
        .collect();
    cash_legs.sort();
    assert_eq!(cash_legs, [-2_000, -1_505]);
    assert!(
        cash["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["payout_id"].is_string())
    );
    let statuses: Vec<String> = sqlx::query_scalar("SELECT DISTINCT status::text FROM conversions")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(statuses, ["paid"]);

    // CSV
    let csv = api.get(&step("export.csv"), &w.admin).await;
    assert_eq!(csv.status, StatusCode::OK);
    assert_eq!(csv.headers[CONTENT_TYPE], "text/csv; charset=utf-8");
    assert_eq!(
        csv.headers[CONTENT_DISPOSITION],
        format!("attachment; filename=\"payouts-{batch}.csv\"")
    );
    assert_eq!(
        csv.body,
        "email,amount,currency\na@x.io,15.05,USD\nb@x.io,20.00,USD\n"
    );

    // list + get
    let list = api
        .get(
            &format!(
                "/api/v1/payouts/batches?status=paid&program_id={}",
                w.program
            ),
            &w.admin,
        )
        .await
        .json();
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["id"], batch);
    assert_eq!(list["items"][0]["total_cents"], 3_505);
    assert_eq!(
        api.get("/api/v1/payouts/batches?status=cancelled", &w.admin)
            .await
            .json()["total"],
        1
    );
    assert_eq!(
        api.get("/api/v1/payouts/batches?status=bogus", &w.admin)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );

    // a refund after payout is a clawback: payable goes negative, so nobody is owed
    assert_eq!(
        api.post(&uri("conversions", o1, "/reverse"), &w.admin)
            .await
            .json()["status"],
        "reversed"
    );
    assert_eq!(balance(&api, &w.admin, w.a.0).await, (0, -1_000));
    assert_balanced(&pool).await;
    assert_eq!(
        api.create_batch(w.program, &w.admin).await.status,
        StatusCode::CONFLICT
    );

    // every mutation was audited
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_log ORDER BY at, id")
        .fetch_all(&pool)
        .await
        .unwrap();
    let want = [
        "conversion.approve",
        "conversion.approve",
        "conversion.approve",
        "payout_batch.create",
        "payout_batch.cancel",
        "payout_batch.create",
        "payout_batch.approve",
        "payout_batch.mark_paid",
        "conversion.reverse",
    ];
    assert_eq!(actions, want);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn money_routes_openapi_declares_query_params_as_query(pool: PgPool) {
    let api = Api::new(&pool);
    let spec = api
        .call(Method::GET, "/api/openapi.json", None, None)
        .await
        .json();
    let mut checked = 0;
    for (path, ops) in spec["paths"].as_object().unwrap() {
        for (method, op) in ops.as_object().unwrap() {
            for p in op["parameters"].as_array().into_iter().flatten() {
                let name = p["name"].as_str().unwrap();
                let templated = path.contains(&format!("{{{name}}}"));
                assert_eq!(
                    p["in"] == "path",
                    templated,
                    "{method} {path}: {name} is `{}`",
                    p["in"]
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 50);
    let params = &spec["paths"]["/api/v1/conversions"]["get"]["parameters"];
    let queries: Vec<&str> = params
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["in"] == "query")
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    for want in [
        "program_id",
        "affiliate_id",
        "status",
        "from",
        "to",
        "page",
        "limit",
    ] {
        assert!(queries.contains(&want), "missing {want} in {queries:?}");
    }
    assert!(spec["paths"]["/api/v1/payouts/batches/{id}/export.csv"]["get"]["responses"]["200"]["content"]["text/csv"].is_object());
}
