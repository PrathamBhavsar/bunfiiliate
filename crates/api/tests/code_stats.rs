//! C30: per-referral-code stats API.

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
const D3: &str = "2026-10-22";

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

struct Api(axum::Router);

impl Api {
    fn new(pool: &PgPool) -> Self {
        Self(app(AppState {
            pool: pool.clone(),
            jwt_keys: api::auth::JwtKeys::single(SECRET).into(),
            version_cache: Default::default(),
            vault: std::sync::Arc::new(bf_core::secrets::Vault::plaintext()),
            secure_cookies: true,
            limiter: Default::default(),
            tracker_url: Default::default(),
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

async fn click(
    pool: &PgPool,
    t: &Tenant,
    aff: Uuid,
    code: &str,
    day: &str,
    country: &str,
    host: Option<&str>,
    fraud: bool,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO clicks (id, program_id, affiliate_id, code, visitor_id, ts, country, referrer_host, fraud_reason) \
         VALUES ($1, $2, $3, $4, 'v', ($5::date + interval '12 hours') AT TIME ZONE 'UTC', $6, $7, \
           CASE WHEN $8 THEN 'bot' END)",
    )
    .bind(id).bind(t.program).bind(aff).bind(code).bind(day).bind(country).bind(host).bind(fraud)
    .execute(pool).await.unwrap();
    id
}

async fn code(pool: &PgPool, t: &Tenant, aff: Uuid, code: &str) {
    sqlx::query("INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) VALUES ($1, $2, $3, 'https://x.io')")
        .bind(code).bind(aff).bind(t.program).execute(pool).await.unwrap();
}

async fn conversion(
    pool: &PgPool,
    t: &Tenant,
    aff: Uuid,
    via: Uuid,
    day: &str,
    status: &str,
    (amount, commission): (i64, i64),
) {
    let (customer, id) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO customers (id, program_id, external_id, affiliate_id, first_click_id) VALUES ($1, $2, $1::text, $3, $4)")
        .bind(customer).bind(t.program).bind(aff).bind(via).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO conversions (id, program_id, customer_id, affiliate_id, external_id, amount_cents, \
           commission_cents, currency, status, occurred_at) \
         VALUES ($1, $2, $3, $4, $1::text, $5, $6, 'USD', $7::conversion_status, \
           ($8::date + interval '12 hours') AT TIME ZONE 'UTC')",
    )
    .bind(id).bind(t.program).bind(customer).bind(aff).bind(amount).bind(commission).bind(status).bind(day)
    .execute(pool).await.unwrap();
}

/// Code `abc` of affiliate A: 3 clicks (1 flagged) on D1, 1 on D2 (none on D1+... gaps zero-filled).
/// Code `other` of A and code `bcode` of B must not leak in.
async fn seed(pool: &PgPool, t: &Tenant) {
    let (a, b) = (t.a.0, t.b.0);
    code(pool, t, a, "abc").await;
    code(pool, t, a, "other").await;
    code(pool, t, b, "bcode").await;
    let c1 = click(pool, t, a, "abc", D1, "US", Some("news.example"), false).await;
    click(pool, t, a, "abc", D1, "US", Some("news.example"), false).await;
    click(pool, t, a, "abc", D1, "DE", None, true).await;
    click(pool, t, a, "abc", D3, "US", Some("blog.example"), false).await;
    let o = click(pool, t, a, "other", D1, "FR", Some("o.example"), false).await;
    let cb = click(pool, t, b, "bcode", D1, "JP", Some("b.example"), false).await;
    conversion(pool, t, a, c1, D1, "approved", (10_000, 1_000)).await;
    conversion(pool, t, a, c1, D3, "pending", (5_000, 500)).await;
    conversion(pool, t, a, c1, D1, "rejected", (9_999, 999)).await;
    conversion(pool, t, a, o, D1, "approved", (7_777, 777)).await;
    conversion(pool, t, b, cb, D1, "approved", (3_333, 333)).await;
}

fn q() -> String {
    format!("from={D1}&to={D3}&granularity=day")
}

#[sqlx::test(migrations = "../db/migrations")]
async fn code_stats_totals_match_seed(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    seed(&pool, &t).await;
    let api = Api::new(&pool);
    let (st, v) = api
        .get(
            &format!("/api/v1/referral-codes/abc/stats?{}", q()),
            &t.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["code"], "abc");
    assert_eq!(v["currency"], "USD");
    assert_eq!(v["from"], D1);
    assert_eq!(v["to"], D3);
    let tot = &v["totals"];
    assert_eq!(
        (
            tot["clicks"].as_i64(),
            tot["flagged_clicks"].as_i64(),
            tot["conversions"].as_i64(),
            tot["revenue_cents"].as_i64(),
            tot["commission_cents"].as_i64()
        ),
        (Some(4), Some(1), Some(2), Some(15_000), Some(1_500)),
    );
    assert_eq!(
        v["top_countries"][0],
        serde_json::json!({"country": "US", "clicks": 3})
    );
    assert_eq!(
        v["top_countries"][1],
        serde_json::json!({"country": "DE", "clicks": 1})
    );
    assert_eq!(
        v["top_referrers"][0],
        serde_json::json!({"host": "news.example", "clicks": 2})
    );
    assert_eq!(v["top_referrers"].as_array().unwrap().len(), 2);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn code_stats_excludes_other_codes(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    seed(&pool, &t).await;
    let api = Api::new(&pool);
    let (_, v) = api
        .get(
            &format!("/api/v1/referral-codes/other/stats?{}", q()),
            &t.admin,
        )
        .await;
    assert_eq!(v["totals"]["clicks"], 1);
    assert_eq!(v["totals"]["commission_cents"], 777);
    assert_eq!(v["top_countries"].as_array().unwrap().len(), 1);
    let s = v.to_string();
    assert!(
        !s.contains("news.example") && !s.contains("b.example"),
        "{s}"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn code_stats_zero_filled_series(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    seed(&pool, &t).await;
    let api = Api::new(&pool);
    let (_, v) = api
        .get(
            &format!("/api/v1/referral-codes/abc/stats?{}", q()),
            &t.admin,
        )
        .await;
    let days: Vec<(&str, i64, i64, i64)> = v["series"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["day"].as_str().unwrap(),
                d["clicks"].as_i64().unwrap(),
                d["conversions"].as_i64().unwrap(),
                d["commission_cents"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        days,
        vec![(D1, 3, 1, 1_000), (D2, 0, 0, 0), (D3, 1, 1, 500)]
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn code_stats_rejects_range_over_366d(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    seed(&pool, &t).await;
    let api = Api::new(&pool);
    for qs in [
        "from=2025-01-01&to=2026-10-22",
        "from=2026-10-22&to=2026-10-20",
        "from=bad",
    ] {
        let (st, _) = api
            .get(&format!("/api/v1/referral-codes/abc/stats?{qs}"), &t.admin)
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{qs}");
    }
    let (st, _) = api
        .get(
            "/api/v1/referral-codes/abc/stats?from=2025-10-22&to=2026-10-22",
            &t.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "365 days back is fine");
    let (st, v) = api.get("/api/v1/referral-codes/abc/stats", &t.admin).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        v["series"].as_array().unwrap().len(),
        30,
        "default is the last 30 days"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn code_stats_unknown_code_404(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    let t2 = tenant(&pool, "b").await;
    seed(&pool, &t).await;
    let api = Api::new(&pool);
    let (st, _) = api.get("/api/v1/referral-codes/nope/stats", &t.admin).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // a code of another workspace looks the same
    let (st, v) = api.get("/api/v1/referral-codes/abc/stats", &t2.admin).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(!v.to_string().contains("abc\""), "{v}");
    // affiliates have no admin route
    let (st, _) = api.get("/api/v1/referral-codes/abc/stats", &t.a.1).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn me_code_stats_foreign_code_404(pool: PgPool) {
    let t = tenant(&pool, "a").await;
    let t2 = tenant(&pool, "b").await;
    seed(&pool, &t).await;
    let api = Api::new(&pool);
    let (st, v) = api
        .get(
            &format!("/api/v1/me/referral-codes/abc/stats?{}", q()),
            &t.a.1,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["totals"]["clicks"], 4);
    // other affiliate, same workspace
    let (st, _) = api.get("/api/v1/me/referral-codes/abc/stats", &t.b.1).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // affiliate of another workspace
    let (st, _) = api
        .get("/api/v1/me/referral-codes/abc/stats", &t2.a.1)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // admins use the other route
    let (st, _) = api
        .get("/api/v1/me/referral-codes/abc/stats", &t.admin)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[test]
fn code_stats_openapi_query_params_are_query() {
    let doc = serde_json::to_value(<api::routes::ApiDoc as utoipa::OpenApi>::openapi()).unwrap();
    for path in [
        "/api/v1/referral-codes/{code}/stats",
        "/api/v1/me/referral-codes/{code}/stats",
    ] {
        let params = doc["paths"][path]["get"]["parameters"].as_array().unwrap();
        for name in ["from", "to", "granularity"] {
            let p = params
                .iter()
                .find(|p| p["name"] == name)
                .unwrap_or_else(|| panic!("{path} {name}"));
            assert_eq!(p["in"], "query", "{path} {name}");
        }
        let c = params.iter().find(|p| p["name"] == "code").unwrap();
        assert_eq!(c["in"], "path");
    }
    for s in [
        "CodeStats",
        "CodeTotals",
        "CodeDay",
        "CodeCountry",
        "CodeReferrer",
    ] {
        assert!(doc["components"]["schemas"].get(s).is_some(), "schema {s}");
    }
}
