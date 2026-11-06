use api::analytics::rollup;
use api::auth::{Principal, issue_access};
use api::{AppState, app};
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST};
use axum::http::{Request, StatusCode};
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-secret";
const D1: &str = "2026-10-20";
const D2: &str = "2026-10-21";

/// A workspace with an admin, one program and affiliates A and B (all with cookies).
struct Tenant {
    admin: String,
    program: Uuid,
    a: (Uuid, String),
    b: (Uuid, String),
}

async fn user(pool: &PgPool, ws: Uuid, role: UserRole) -> (Uuid, String) {
    let u = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: format!("{}@x.io", Uuid::now_v7()),
        password_hash: "unused".into(),
        role,
    };
    create_user(pool, &u).await.unwrap();
    let token = issue_access(SECRET, &Principal::from(&u)).unwrap();
    (u.id, format!("bf_at={token}"))
}

async fn affiliate(pool: &PgPool, ws: Uuid) -> (Uuid, String) {
    let (user_id, cookie) = user(pool, ws, UserRole::Affiliate).await;
    let id = Uuid::now_v7();
    bf_db::affiliates::insert_affiliate(pool, id, ws, user_id, None)
        .await
        .unwrap();
    (id, cookie)
}

async fn tenant(pool: &PgPool, slug: &str) -> Tenant {
    let ws = Uuid::now_v7();
    create_workspace(pool, ws, slug, slug).await.unwrap();
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
    Tenant {
        admin: user(pool, ws, UserRole::Admin).await.1,
        program,
        a: affiliate(pool, ws).await,
        b: affiliate(pool, ws).await,
    }
}

type Dims<'a> = (&'a str, &'a str, &'a str, &'a str, Option<&'a str>);

/// Insert a click at noon UTC on `day`; returns its id.
async fn click(pool: &PgPool, t: &Tenant, aff: Uuid, day: &str, d: Dims<'_>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO clicks (id, program_id, affiliate_id, code, visitor_id, ts, country, os, \
           browser, device, referrer_host) \
         VALUES ($1, $2, $3, 'code', 'v', ($4::date + interval '12 hours') AT TIME ZONE 'UTC', \
           $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(t.program)
    .bind(aff)
    .bind(day)
    .bind(d.0)
    .bind(d.1)
    .bind(d.2)
    .bind(d.3)
    .bind(d.4)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// A customer (attributed to `via` when given) with one conversion at noon UTC on `day`.
async fn conversion(
    pool: &PgPool,
    t: &Tenant,
    aff: Uuid,
    via: Option<Uuid>,
    status: &str,
    (amount, commission): (i64, i64),
) -> Uuid {
    let (customer, id) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO customers (id, program_id, external_id, affiliate_id, first_click_id) \
         VALUES ($1, $2, $1::text, $3, $4)",
    )
    .bind(customer)
    .bind(t.program)
    .bind(aff)
    .bind(via)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO conversions (id, program_id, customer_id, affiliate_id, external_id, \
           amount_cents, commission_cents, currency, status, occurred_at) \
         VALUES ($1, $2, $3, $4, $1::text, $5, $6, 'USD', $7::conversion_status, \
           ($8::date + interval '12 hours') AT TIME ZONE 'UTC')",
    )
    .bind(id)
    .bind(t.program)
    .bind(customer)
    .bind(aff)
    .bind(amount)
    .bind(commission)
    .bind(status)
    .bind(D1)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// A: 4 clicks, 2 live conversions (+1 rejected). B: 2 clicks, 1 conversion.
async fn seed(pool: &PgPool, t: &Tenant) {
    let (a, b) = (t.a.0, t.b.0);
    let c1 = click(
        pool,
        t,
        a,
        D1,
        ("US", "mac", "chrome", "desktop", Some("news.com")),
    )
    .await;
    click(
        pool,
        t,
        a,
        D1,
        ("US", "mac", "safari", "mobile", Some("news.com")),
    )
    .await;
    click(pool, t, a, D1, ("DE", "win", "firefox", "desktop", None)).await;
    click(
        pool,
        t,
        a,
        D2,
        ("US", "mac", "chrome", "desktop", Some("=evil,\"x\"")),
    )
    .await;
    let c5 = click(
        pool,
        t,
        b,
        D1,
        ("DE", "win", "chrome", "desktop", Some("news.com")),
    )
    .await;
    click(pool, t, b, D1, ("US", "linux", "firefox", "desktop", None)).await;
    conversion(pool, t, a, Some(c1), "approved", (10_000, 1_000)).await;
    conversion(pool, t, a, None, "pending", (5_000, 500)).await;
    conversion(pool, t, a, Some(c1), "rejected", (9_999, 999)).await;
    conversion(pool, t, b, Some(c5), "paid", (2_000, 200)).await;
}

struct Api(axum::Router);

impl Api {
    fn new(pool: &PgPool) -> Self {
        Self(app(AppState {
            pool: pool.clone(),
            jwt_secret: SECRET.into(),
            vault: std::sync::Arc::new(bf_core::secrets::Vault::plaintext()),
            secure_cookies: true,
            limiter: Default::default(),
        }))
    }

    async fn raw(&self, uri: &str, cookie: &str) -> (StatusCode, String, Option<String>) {
        let req = Request::builder()
            .uri(uri)
            .header(HOST, "bf.test")
            .header(COOKIE, cookie)
            .body(Body::empty())
            .unwrap();
        let res = self.0.clone().oneshot(req).await.unwrap();
        let (status, ct) = (
            res.status(),
            res.headers()
                .get(CONTENT_TYPE)
                .map(|v| v.to_str().unwrap().to_string()),
        );
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap(), ct)
    }

    async fn get(&self, uri: &str, cookie: &str) -> (StatusCode, Value) {
        let (st, body, _) = self.raw(uri, cookie).await;
        (st, serde_json::from_str(&body).unwrap_or(Value::Null))
    }
}

async fn snapshot(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT concat_ws('|', program_id, affiliate_id, day, dim, dim_value, clicks, conversions, \
           revenue_cents, commission_cents) FROM daily_rollups ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

fn range() -> String {
    format!("from={D1}&to={D2}")
}

#[sqlx::test(migrations = "../db/migrations")]
async fn analytics_rollup_idempotent(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    seed(&pool, &t).await;

    assert!(rollup(&pool).await.unwrap() > 0);
    let first = snapshot(&pool).await;
    assert!(!first.is_empty());
    rollup(&pool).await.unwrap();
    assert_eq!(snapshot(&pool).await, first);

    // incremental: a late click on the watermark day and a rejection inside the window
    click(
        &pool,
        &t,
        t.b.0,
        D2,
        ("FR", "mac", "chrome", "desktop", None),
    )
    .await;
    sqlx::query("UPDATE conversions SET status = 'rejected' WHERE amount_cents = 5000")
        .execute(&pool)
        .await
        .unwrap();
    rollup(&pool).await.unwrap();
    let api = Api::new(&pool);
    let (_, s) = api
        .get(&format!("/api/v1/analytics/summary?{}", range()), &t.admin)
        .await;
    assert_eq!(s["clicks"], 7);
    assert_eq!(s["conversions"], 2);
    assert_eq!(s["revenue_cents"], 12_000);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn analytics_breakdown_sums_equal_totals(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    seed(&pool, &t).await;
    rollup(&pool).await.unwrap();
    let api = Api::new(&pool);

    let (st, total) = api
        .get(&format!("/api/v1/analytics/summary?{}", range()), &t.admin)
        .await;
    assert_eq!(st, StatusCode::OK, "{total}");
    assert_eq!(total["clicks"], 6);
    assert_eq!(total["conversions"], 3);
    assert_eq!(total["revenue_cents"], 17_000);
    assert_eq!(total["commission_cents"], 1_700);
    assert_eq!(total["conversion_rate_bps"], 5_000);
    assert_eq!(total["breakdown"].as_array().unwrap().len(), 0);

    for dim in ["total", "country", "os", "browser", "device", "referrer"] {
        let uri = format!("/api/v1/analytics/summary?{}&dim={dim}", range());
        let (_, s) = api.get(&uri, &t.admin).await;
        let rows = s["breakdown"].as_array().unwrap();
        assert!(!rows.is_empty(), "{dim}");
        for k in ["clicks", "conversions", "revenue_cents", "commission_cents"] {
            let sum: i64 = rows.iter().map(|r| r[k].as_i64().unwrap()).sum();
            assert_eq!(sum, total[k].as_i64().unwrap(), "{dim} {k}");
        }
    }
    let (_, s) = api
        .get(
            &format!("/api/v1/analytics/summary?{}&dim=country", range()),
            &t.admin,
        )
        .await;
    let us = &s["breakdown"][0];
    assert_eq!(
        (us["dim_value"].as_str(), us["clicks"].as_i64()),
        (Some("US"), Some(4))
    );
    assert_eq!(us["conversion_rate_bps"], 2_500);

    let (_, ts) = api
        .get(
            &format!("/api/v1/analytics/timeseries?from={D1}&to=2026-10-22"),
            &t.admin,
        )
        .await;
    let days: Vec<_> = ts
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["day"].as_str().unwrap(),
                p["clicks"].as_i64().unwrap(),
                p["conversions"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(days, [(D1, 5, 3), (D2, 1, 0), ("2026-10-22", 0, 0)]);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn analytics_affiliate_sees_only_own(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    seed(&pool, &t).await;
    rollup(&pool).await.unwrap();
    let api = Api::new(&pool);

    let own = format!("/api/v1/analytics/summary?{}", range());
    let (st, s) = api.get(&own, &t.a.1).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        (s["clicks"].as_i64(), s["conversions"].as_i64()),
        (Some(4), Some(2))
    );
    assert_eq!(s["revenue_cents"], 15_000);
    let (_, s) = api.get(&own, &t.b.1).await;
    assert_eq!(
        (s["clicks"].as_i64(), s["revenue_cents"].as_i64()),
        (Some(2), Some(2_000))
    );

    // naming another affiliate is 404 on every route; naming oneself is fine
    let (a, b) = (t.a.0, t.b.0);
    for path in [
        "/api/v1/analytics/summary",
        "/api/v1/analytics/timeseries",
        "/api/v1/reports/analytics.csv",
    ] {
        let uri = format!("{path}?{}&affiliate_id={b}", range());
        assert_eq!(
            api.get(&uri, &t.a.1).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
        let uri = format!("{path}?{}&affiliate_id={a}", range());
        assert_eq!(api.get(&uri, &t.a.1).await.0, StatusCode::OK, "{path}");
    }
    let (_, csv, _) = api
        .raw(
            &format!("/api/v1/reports/analytics.csv?{}", range()),
            &t.a.1,
        )
        .await;
    assert!(
        csv.ends_with("2026-10-20,total,,3,2,15000,1500,6666\n2026-10-21,total,,1,0,0,0,0\n"),
        "{csv}"
    );

    // an admin can narrow to one affiliate
    let (_, s) = api.get(&format!("{own}&affiliate_id={b}"), &t.admin).await;
    assert_eq!(s["clicks"], 2);
    assert_eq!(
        api.get(&own, "bf_at=junk").await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn analytics_workspace_isolation(pool: PgPool) {
    let (t1, t2) = (tenant(&pool, "a").await, tenant(&pool, "b").await);
    seed(&pool, &t1).await;
    click(
        &pool,
        &t2,
        t2.a.0,
        D1,
        ("JP", "mac", "chrome", "desktop", None),
    )
    .await;
    rollup(&pool).await.unwrap();
    let api = Api::new(&pool);

    let uri = format!("/api/v1/analytics/summary?{}", range());
    assert_eq!(api.get(&uri, &t1.admin).await.1["clicks"], 6);
    assert_eq!(api.get(&uri, &t2.admin).await.1["clicks"], 1);

    for path in [
        "/api/v1/analytics/summary",
        "/api/v1/analytics/timeseries",
        "/api/v1/reports/analytics.csv",
    ] {
        let foreign = format!("{path}?{}&program_id={}", range(), t1.program);
        assert_eq!(
            api.get(&foreign, &t2.admin).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
        assert_eq!(
            api.get(&foreign, &t2.a.1).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
        let own = format!("{path}?{}&program_id={}", range(), t1.program);
        assert_eq!(api.get(&own, &t1.admin).await.0, StatusCode::OK, "{path}");
    }
    // another workspace's affiliate id yields nothing
    let uri = format!("{uri}&affiliate_id={}", t1.a.0);
    assert_eq!(api.get(&uri, &t2.admin).await.1["clicks"], 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn analytics_csv_exact(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    seed(&pool, &t).await;
    rollup(&pool).await.unwrap();
    let api = Api::new(&pool);
    let head =
        "day,dim,dim_value,clicks,conversions,revenue_cents,commission_cents,conversion_rate_bps\n";

    let uri = format!("/api/v1/reports/analytics.csv?{}&dim=country", range());
    let (st, csv, ct) = api.raw(&uri, &t.admin).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ct.as_deref(), Some("text/csv; charset=utf-8"));
    assert_eq!(
        csv,
        format!(
            "{head}\
             2026-10-20,country,,0,1,5000,500,0\n\
             2026-10-20,country,DE,2,1,2000,200,5000\n\
             2026-10-20,country,US,3,1,10000,1000,3333\n\
             2026-10-21,country,US,1,0,0,0,0\n"
        )
    );

    // the default dim is total; spreadsheet formulas are defused and quoting is RFC 4180
    let uri = format!("/api/v1/reports/analytics.csv?{}", range());
    let (_, csv, _) = api.raw(&uri, &t.admin).await;
    assert_eq!(
        csv,
        format!("{head}2026-10-20,total,,5,3,17000,1700,6000\n2026-10-21,total,,1,0,0,0,0\n")
    );
    let uri = format!("/api/v1/reports/analytics.csv?{}&dim=referrer", range());
    let (_, csv, _) = api.raw(&uri, &t.admin).await;
    assert_eq!(
        csv,
        format!(
            "{head}\
             2026-10-20,referrer,,2,1,5000,500,5000\n\
             2026-10-20,referrer,news.com,3,2,12000,1200,6666\n\
             2026-10-21,referrer,\"'=evil,\"\"x\"\"\",1,0,0,0,0\n"
        )
    );

    let bad = format!("/api/v1/reports/analytics.csv?from=nope&to={D2}");
    assert_eq!(api.raw(&bad, &t.admin).await.0, StatusCode::BAD_REQUEST);
    let backwards = format!("/api/v1/reports/analytics.csv?from={D2}&to={D1}");
    assert_eq!(
        api.raw(&backwards, &t.admin).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn analytics_rollup_picks_up_a_late_reversal_and_a_back_dated_sale(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    seed(&pool, &t).await;
    rollup(&pool).await.unwrap();
    let api = Api::new(&pool);
    let uri = "/api/v1/analytics/summary?from=2026-10-01&to=2026-10-21";
    let (_, before) = api.get(uri, &t.admin).await;
    assert_eq!(before["conversions"], 3);

    // A chargeback weeks after the sale: far outside the one-day lookback.
    sqlx::query(
        "UPDATE conversions SET occurred_at = '2026-10-05T12:00:00Z' WHERE amount_cents = 2000",
    )
    .execute(&pool)
    .await
    .unwrap();
    rollup(&pool).await.unwrap();
    let (_, moved) = api.get(uri, &t.admin).await;
    assert_eq!(
        moved["conversions"], 3,
        "back-dating moves it, still counted"
    );

    sqlx::query(
        "UPDATE conversions SET status = 'reversed', reversed_at = now() WHERE amount_cents = 2000",
    )
    .execute(&pool)
    .await
    .unwrap();
    rollup(&pool).await.unwrap();
    let (_, after) = api.get(uri, &t.admin).await;
    assert_eq!(after["conversions"], 2);
    assert_eq!(after["revenue_cents"], 15_000);

    // A back-dated sale arriving now is folded in as well.
    let c = conversion(&pool, &t, t.b.0, None, "approved", (700, 70)).await;
    sqlx::query("UPDATE conversions SET occurred_at = '2026-10-03T12:00:00Z' WHERE id = $1")
        .bind(c)
        .execute(&pool)
        .await
        .unwrap();
    rollup(&pool).await.unwrap();
    let (_, late) = api.get(uri, &t.admin).await;
    assert_eq!(late["conversions"], 3);
    assert_eq!(late["revenue_cents"], 15_700);
}
