//! B06: cached exchange rates, the daily rate tick, and `/analytics/summary-all`.

use std::sync::Arc;

use api::auth::hash_password;
use api::fx::{self, FxQuote, RateSource};
use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{Request, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
use bf_core::{Currency, Rate};
use chrono::NaiveDate;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery";

fn app(pool: PgPool) -> Router {
    let ring = KeyRing::from_pairs("t1", &[("t1", [7u8; 32])]).unwrap();
    let state = AppState {
        pool,
        jwt_keys: api::auth::JwtKeys::single("test-secret").into(),
        version_cache: Default::default(),
        tracker_url: Default::default(),
        vault: Arc::new(Vault::new(ring)),
        secure_cookies: true,
        limiter: Default::default(),
    };
    build(routes::routes(), state)
}

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

struct Ws {
    id: Uuid,
    cookie: String,
    affiliate: Uuid,
}

async fn workspace(pool: &PgPool, app: &Router, role: &str) -> Ws {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', $2)")
        .bind(id)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
    let email = format!("{id}@x.io");
    sqlx::query(
        "INSERT INTO users (id, workspace_id, email, password_hash, role) \
         VALUES ($1, $2, $3, $4, $5::user_role)",
    )
    .bind(Uuid::now_v7())
    .bind(id)
    .bind(&email)
    .bind(hash_password(PASSWORD).unwrap())
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
    let (st, cookies) = login(app, &email).await;
    assert_eq!(st, StatusCode::OK);
    // the affiliate that owns the seeded rollups
    let (au, aff) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO users (id, workspace_id, email, password_hash, role) \
         VALUES ($1, $2, $3, 'h', 'affiliate')",
    )
    .bind(au)
    .bind(id)
    .bind(format!("{au}@x.io"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO affiliates (id, workspace_id, user_id, status) VALUES ($1, $2, $3, 'active')",
    )
    .bind(aff)
    .bind(id)
    .bind(au)
    .execute(pool)
    .await
    .unwrap();
    Ws {
        id,
        cookie: cookies,
        affiliate: aff,
    }
}

async fn login(app: &Router, email: &str) -> (StatusCode, String) {
    let (st, _, cookies) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        json!({"email": email, "password": PASSWORD}),
    )
    .await;
    let c = cookies
        .iter()
        .find(|c| c.starts_with("bf_at="))
        .map(|c| c.split(';').next().unwrap().to_string())
        .unwrap_or_default();
    (st, c)
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
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

async fn program(pool: &PgPool, ws: Uuid, currency: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
         VALUES ($1, $2, $3, $4, $5, 's')",
    )
    .bind(id)
    .bind(ws)
    .bind(format!("p-{currency}"))
    .bind(currency)
    .bind(id.to_string())
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn rollup(pool: &PgPool, w: &Ws, program: Uuid, d: &str, revenue: i64, commission: i64) {
    sqlx::query(
        "INSERT INTO daily_rollups (program_id, affiliate_id, day, dim, dim_value, clicks, \
           conversions, revenue_cents, commission_cents) \
         VALUES ($1, $2, $3, 'total', '', 10, 1, $4, $5)",
    )
    .bind(program)
    .bind(w.affiliate)
    .bind(day(d))
    .bind(revenue)
    .bind(commission)
    .execute(pool)
    .await
    .unwrap();
}

/// USD -> `quote` rate on `as_of`.
async fn rate(pool: &PgPool, quote: &str, r: &str, as_of: &str) {
    sqlx::query(
        "INSERT INTO exchange_rates (base, quote, rate, as_of) VALUES ('USD', $1, $2::numeric, $3)",
    )
    .bind(quote)
    .bind(r)
    .bind(day(as_of))
    .execute(pool)
    .await
    .unwrap();
}

fn cur(c: &str) -> Currency {
    Currency::new(c).unwrap()
}

fn quote(date: &str, rates: &[(&str, &str)]) -> RateSource {
    RateSource::Mock(Ok(FxQuote {
        date: day(date),
        rates: rates
            .iter()
            .map(|(c, r)| (cur(c), Rate::parse(r).unwrap()))
            .collect(),
    }))
}

async fn rows(pool: &PgPool) -> Vec<(String, String, String, NaiveDate)> {
    sqlx::query_as("SELECT base::text, quote::text, rate::text, as_of FROM exchange_rates ORDER BY quote, as_of")
        .fetch_all(pool)
        .await
        .unwrap()
}

fn summary_uri(q: &str) -> String {
    format!("/api/v1/analytics/summary-all?{q}")
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multi_currency_rate_tick_upserts_only_pairs_in_use(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "admin").await;
    program(&pool, w.id, "USD").await;
    program(&pool, w.id, "EUR").await;
    program(&pool, w.id, "EUR").await;
    // The source offers more than was asked for; only what is in use is stored.
    let src = quote(
        "2026-11-06",
        &[("EUR", "0.89566"), ("JPY", "151.1"), ("GBP", "0.74711")],
    );
    assert_eq!(fx::tick(&pool, &src).await.unwrap(), 1);
    assert_eq!(
        rows(&pool).await,
        [(
            "USD".into(),
            "EUR".into(),
            "0.89566000".into(),
            day("2026-11-06")
        )]
    );
    // idempotent for the same day, and a new day adds a row
    assert_eq!(fx::tick(&pool, &src).await.unwrap(), 1);
    assert_eq!(rows(&pool).await.len(), 1);
    let src = quote("2026-11-09", &[("EUR", "0.9")]);
    fx::tick(&pool, &src).await.unwrap();
    assert_eq!(rows(&pool).await.len(), 2);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multi_currency_rate_tick_without_foreign_programs_makes_no_call(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "admin").await;
    program(&pool, w.id, "USD").await;
    // would fail if it were called
    let src = RateSource::Mock(Err(fx::FxError::Unavailable("must not be called".into())));
    assert_eq!(fx::tick(&pool, &src).await.unwrap(), 0);
    assert!(rows(&pool).await.is_empty());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multi_currency_fetch_failure_falls_back_to_last_cached_not_1to1(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "admin").await;
    program(&pool, w.id, "EUR").await;
    rate(&pool, "EUR", "0.9", "2026-11-05").await;
    let before = rows(&pool).await;
    let down = RateSource::Mock(Err(fx::FxError::Unavailable("down".into())));
    assert!(fx::tick(&pool, &down).await.is_err());
    assert_eq!(
        rows(&pool).await,
        before,
        "failed tick must not touch the cache"
    );
    let one: i64 = sqlx::query_scalar("SELECT count(*) FROM exchange_rates WHERE rate = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(one, 0);
    // a quote that omits the currency also writes nothing
    assert_eq!(
        fx::tick(&pool, &quote("2026-11-06", &[("GBP", "0.7")]))
            .await
            .unwrap(),
        0
    );
    assert_eq!(rows(&pool).await, before);

    // the read side keeps converting with the stale row
    let usd = program(&pool, w.id, "USD").await;
    rollup(&pool, &w, usd, "2026-11-08", 10_000, 1_000).await;
    let eur = sqlx::query_scalar("SELECT id FROM programs WHERE currency = 'EUR'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let eur: Uuid = eur;
    rollup(&pool, &w, eur, "2026-11-08", 9_000, 900).await;
    let (st, body, _) = call(
        &app,
        "GET",
        &summary_uri("display_currency=USD&from=2026-11-01&to=2026-11-08"),
        Some(&w.cookie),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    // 9000 / 0.9 = 10000 -> 20000 in all
    assert_eq!(body["revenue_cents"], 20_000);
    assert_eq!(body["unconverted"], json!([]));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multi_currency_summary_converts_using_latest_rate_leq_date(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "admin").await;
    let eur = program(&pool, w.id, "EUR").await;
    rate(&pool, "EUR", "0.5", "2026-11-02").await;
    rate(&pool, "EUR", "0.8", "2026-11-06").await;
    rate(&pool, "EUR", "0.25", "2026-11-20").await; // future: must never be used
    rollup(&pool, &w, eur, "2026-11-01", 100, 10).await; // before any rate: unconverted
    rollup(&pool, &w, eur, "2026-11-04", 1_000, 100).await; // 0.5 -> 2000, 200
    rollup(&pool, &w, eur, "2026-11-06", 800, 80).await; // 0.8 -> 1000, 100
    rollup(&pool, &w, eur, "2026-11-10", 800, 80).await; // still 0.8 -> 1000, 100
    let (st, b, _) = call(
        &app,
        "GET",
        &summary_uri("display_currency=USD&from=2026-11-01&to=2026-11-15"),
        Some(&w.cookie),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["display_currency"], "USD");
    assert_eq!(b["revenue_cents"], 4_000);
    assert_eq!(b["commission_cents"], 400);
    assert_eq!(
        b["unconverted"],
        json!([{"program_id": eur, "currency": "EUR", "revenue_cents": 100, "commission_cents": 10}])
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multi_currency_summary_triangulates_cross_rates_through_usd(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "admin").await;
    let (eur, gbp, usd) = (
        program(&pool, w.id, "EUR").await,
        program(&pool, w.id, "GBP").await,
        program(&pool, w.id, "USD").await,
    );
    rate(&pool, "EUR", "0.89566", "2026-11-06").await;
    rate(&pool, "GBP", "0.74711", "2026-11-06").await;
    rollup(&pool, &w, eur, "2026-11-06", 10_000, 1_000).await;
    rollup(&pool, &w, gbp, "2026-11-06", 10_000, 1_000).await;
    rollup(&pool, &w, usd, "2026-11-06", 10_000, 1_000).await;
    // display GBP: EUR 10000 -> 8341 (one rounding), GBP identity 10000, USD 10000 -> 7471
    let (st, b, _) = call(
        &app,
        "GET",
        &summary_uri("display_currency=GBP&from=2026-11-06&to=2026-11-06"),
        Some(&w.cookie),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["revenue_cents"], 8_341 + 10_000 + 7_471);
    // 1000 * 0.74711 / 0.89566 = 834.1 -> 834; 1000 -> 1000; 1000 * 0.74711 = 747.11 -> 747
    assert_eq!(b["commission_cents"], 834 + 1_000 + 747);
    assert_eq!(b["unconverted"], json!([]));
    // display USD: EUR 10000 / 0.89566 = 11164.8 -> 11165 ; GBP 10000 / 0.74711 = 13385.1 -> 13385
    let (_, b, _) = call(
        &app,
        "GET",
        &summary_uri("display_currency=usd&from=2026-11-06&to=2026-11-06"),
        Some(&w.cookie),
        Value::Null,
    )
    .await;
    assert_eq!(b["revenue_cents"], 11_165 + 13_385 + 10_000);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multi_currency_summary_excludes_and_lists_unconverted_when_rate_missing(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "admin").await;
    let (usd, eur, jpy) = (
        program(&pool, w.id, "USD").await,
        program(&pool, w.id, "EUR").await,
        program(&pool, w.id, "JPY").await,
    );
    rate(&pool, "EUR", "0.5", "2026-11-06").await;
    rollup(&pool, &w, usd, "2026-11-06", 1_000, 100).await;
    rollup(&pool, &w, eur, "2026-11-06", 500, 50).await;
    rollup(&pool, &w, jpy, "2026-11-06", 77_700, 7_770).await;
    let (st, b, _) = call(
        &app,
        "GET",
        &summary_uri("display_currency=USD&from=2026-11-06&to=2026-11-06"),
        Some(&w.cookie),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["revenue_cents"], 2_000, "jpy must not be summed 1:1");
    assert_eq!(b["commission_cents"], 200);
    assert_eq!(
        b["unconverted"],
        json!([{"program_id": jpy, "currency": "JPY", "revenue_cents": 77_700, "commission_cents": 7_770}])
    );
    // a display currency with no cached rate: only same-currency programs convert
    let (_, b, _) = call(
        &app,
        "GET",
        &summary_uri("display_currency=CHF&from=2026-11-06&to=2026-11-06"),
        Some(&w.cookie),
        Value::Null,
    )
    .await;
    assert_eq!(b["revenue_cents"], 0);
    assert_eq!(b["unconverted"].as_array().unwrap().len(), 3);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multi_currency_summary_is_admin_only_workspace_scoped_and_validated(pool: PgPool) {
    let app = app(pool.clone());
    let (w1, w2) = (
        workspace(&pool, &app, "admin").await,
        workspace(&pool, &app, "admin").await,
    );
    let p = program(&pool, w1.id, "USD").await;
    rollup(&pool, &w1, p, "2026-11-06", 1_000, 100).await;
    let q = "display_currency=USD&from=2026-11-06&to=2026-11-06";
    let (_, b, _) = call(&app, "GET", &summary_uri(q), Some(&w2.cookie), Value::Null).await;
    assert_eq!(
        (
            b["revenue_cents"].as_i64(),
            b["unconverted"].as_array().map(Vec::len)
        ),
        (Some(0), Some(0))
    );
    let (_, b, _) = call(&app, "GET", &summary_uri(q), Some(&w1.cookie), Value::Null).await;
    assert_eq!(b["revenue_cents"], 1_000);
    // display_currency defaults to USD
    let (_, b, _) = call(
        &app,
        "GET",
        &summary_uri("from=2026-11-06&to=2026-11-06"),
        Some(&w1.cookie),
        Value::Null,
    )
    .await;
    assert_eq!(
        (b["display_currency"].as_str(), b["revenue_cents"].as_i64()),
        (Some("USD"), Some(1_000))
    );
    for bad in [
        "display_currency=",
        "display_currency=US",
        "display_currency=USDX",
        "display_currency=U$D",
        "display_currency=USD&from=nope",
        "display_currency=USD&from=2026-11-06&to=2026-01-01",
    ] {
        let (st, ..) = call(
            &app,
            "GET",
            &summary_uri(bad),
            Some(&w1.cookie),
            Value::Null,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (st, ..) = call(&app, "GET", &summary_uri(q), None, Value::Null).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multi_currency_summary_forbidden_for_affiliates(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "affiliate").await;
    let (st, ..) = call(
        &app,
        "GET",
        &summary_uri("display_currency=USD"),
        Some(&w.cookie),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multi_currency_exchange_rates_reject_bad_rows(pool: PgPool) {
    for (rate, base, quote) in [
        ("0", "USD", "EUR"),
        ("-1", "USD", "EUR"),
        ("1", "USD", "USD"),
    ] {
        let r = sqlx::query("INSERT INTO exchange_rates (base, quote, rate, as_of) VALUES ($1, $2, $3::numeric, '2026-11-06')")
            .bind(base)
            .bind(quote)
            .bind(rate)
            .execute(&pool)
            .await;
        assert!(r.is_err(), "{rate} {base}->{quote}");
    }
}
