//! B05: tax form capture, 1099-style CSV, per-payout invoice.

use std::sync::Arc;

use api::auth::hash_password;
use api::payouts::{approve_batch, create_batch, mark_paid};
use api::worker::{approve_due, process_events};
use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{Request, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
use bf_db::tracker::{InboundEvent, insert_events};
use chrono::{Duration, Utc};
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

/// A workspace with a USD program (10% lifetime, auto-approve, no hold) and an admin.
struct Ws {
    id: Uuid,
    program: Uuid,
    admin: Uuid,
    admin_cookie: String,
}

struct Aff {
    id: Uuid,
    cookie: String,
    code: String,
}

async fn login(app: &Router, email: &str) -> String {
    let (st, _, cookies) = raw(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        json!({"email": email, "password": PASSWORD}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    cookies
        .iter()
        .find(|c| c.starts_with("bf_at="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn make_user(pool: &PgPool, ws: Uuid, role: &str) -> (Uuid, String) {
    let id = Uuid::now_v7();
    let email = format!("{id}@x.io");
    sqlx::query(
        "INSERT INTO users (id, workspace_id, email, password_hash, role) \
         VALUES ($1, $2, $3, $4, $5::user_role)",
    )
    .bind(id)
    .bind(ws)
    .bind(&email)
    .bind(hash_password(PASSWORD).unwrap())
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
    (id, email)
}

async fn workspace(pool: &PgPool, app: &Router, name: &str) -> Ws {
    let (id, program, plan) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, $3, $2)")
        .bind(id)
        .bind(id.to_string())
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, auto_approve, hold_days, \
           public_key, webhook_secret) VALUES ($1, $2, 'p', 'USD', true, 0, $3, 's')",
    )
    .bind(program)
    .bind(id)
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
    let (admin, email) = make_user(pool, id, "admin").await;
    let admin_cookie = login(app, &email).await;
    Ws {
        id,
        program,
        admin,
        admin_cookie,
    }
}

async fn affiliate(pool: &PgPool, app: &Router, w: &Ws, email: &str) -> Aff {
    let (user, login_email) = make_user(pool, w.id, "affiliate").await;
    let (id, code) = (Uuid::now_v7(), format!("C{}", Uuid::now_v7().simple()));
    sqlx::query(
        "INSERT INTO affiliates (id, workspace_id, user_id, status, payout_email) \
         VALUES ($1, $2, $3, 'active', $4)",
    )
    .bind(id)
    .bind(w.id)
    .bind(user)
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO enrollments (affiliate_id, program_id) VALUES ($1, $2)")
        .bind(id)
        .bind(w.program)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) \
         VALUES ($1, $2, $3, 'https://x.io/')",
    )
    .bind(&code)
    .bind(id)
    .bind(w.program)
    .execute(pool)
    .await
    .unwrap();
    Aff {
        id,
        cookie: login(app, &login_email).await,
        code,
    }
}

async fn sale(pool: &PgPool, w: &Ws, a: &Aff, order: &str, cents: i64) {
    let n = insert_events(
        pool,
        &[InboundEvent {
            id: Uuid::now_v7(),
            program_id: w.program,
            source: "generic".into(),
            external_id: order.into(),
            payload: json!({"customer": order, "amount_cents": cents, "code": a.code,
                "occurred_at": (Utc::now() - Duration::hours(1)).to_rfc3339()}),
            received_ms: Utc::now().timestamp_millis(),
        }],
    )
    .await
    .unwrap();
    assert_eq!(n, 1);
    process_events(pool).await.unwrap();
}

/// Approve everything owed and pay one batch; returns the batch id.
async fn pay(pool: &PgPool, w: &Ws) -> Uuid {
    approve_due(pool).await.unwrap();
    let batch = create_batch(pool, w.id, w.program, w.admin)
        .await
        .unwrap()
        .unwrap();
    approve_batch(pool, w.id, batch, w.admin).await.unwrap();
    mark_paid(pool, w.id, batch).await.unwrap();
    batch
}

async fn payout_of(pool: &PgPool, batch: Uuid, aff: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM payouts WHERE batch_id = $1 AND affiliate_id = $2")
        .bind(batch)
        .bind(aff)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// (status, json body, set-cookie values); the body is `Null` when it is not JSON.
async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
) -> (StatusCode, Value, Vec<String>) {
    let (st, bytes, _, cookies) = raw_bytes(app, method, uri, cookie, body).await;
    (
        st,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookies,
    )
}

async fn raw_bytes(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
) -> (StatusCode, Vec<u8>, String, Vec<String>) {
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
    let ctype = res
        .headers()
        .get(CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let cookies = res
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (st, bytes.to_vec(), ctype, cookies)
}

async fn text(app: &Router, uri: &str, cookie: &str) -> (StatusCode, String, String) {
    let (st, bytes, ctype, _) = raw_bytes(app, "GET", uri, Some(cookie), Value::Null).await;
    (st, String::from_utf8(bytes).unwrap(), ctype)
}

fn form(kind: &str, name: &str, country: &str, last4: &str) -> Value {
    json!({"kind": kind, "legal_name": name, "country": country, "tax_id_last4": last4})
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_form_rejects_non_4digit_id(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "w").await;
    let a = affiliate(&pool, &app, &w, "a@x.io").await;
    for bad in ["12", "", "12a4", "١٢٣٤", " 123"] {
        let (st, ..) = raw(
            &app,
            "PUT",
            "/api/v1/me/tax-form",
            Some(&a.cookie),
            form("w9", "A", "US", bad),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    let kind: String =
        sqlx::query_scalar("SELECT tax_form_kind::text FROM affiliates WHERE id = $1")
            .bind(a.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kind, "none");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_form_rejects_full_id_looking_value(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "w").await;
    let a = affiliate(&pool, &app, &w, "a@x.io").await;
    for bad in ["123456789", "123-45-6789", "12-3456789", "1234567890123"] {
        let (st, body, _) = raw(
            &app,
            "PUT",
            "/api/v1/me/tax-form",
            Some(&a.cookie),
            form("w9", "A", "US", bad),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
        assert!(!body.to_string().contains(bad), "echoed the value");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_form_other_invalid_input_400(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "w").await;
    let a = affiliate(&pool, &app, &w, "a@x.io").await;
    for body in [
        form("none", "A", "US", "1234"),
        form("w3", "A", "US", "1234"),
        form("w9", "   ", "US", "1234"),
        form("w9", &"x".repeat(201), "US", "1234"),
        form("w9", "A", "USA", "1234"),
        form("w9", "A", "U1", "1234"),
        form("w9", "A", "", "1234"),
    ] {
        let (st, ..) = raw(
            &app,
            "PUT",
            "/api/v1/me/tax-form",
            Some(&a.cookie),
            body.clone(),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_form_saved_and_readable(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "w").await;
    let a = affiliate(&pool, &app, &w, "a@x.io").await;
    let (st, body, _) = raw(
        &app,
        "GET",
        "/api/v1/me/tax-form",
        Some(&a.cookie),
        Value::Null,
    )
    .await;
    assert_eq!((st, body["kind"].as_str()), (StatusCode::OK, Some("none")));
    // tax id is optional; country is normalised to upper case; the name is trimmed
    let (st, body, _) = raw(
        &app,
        "PUT",
        "/api/v1/me/tax-form",
        Some(&a.cookie),
        json!({"kind": "w8", "legal_name": "  Ada Lovelace ", "country": "gb"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["legal_name"], "Ada Lovelace");
    assert_eq!(body["country"], "GB");
    assert!(body["tax_id_last4"].is_null());
    let (st, body, _) = raw(
        &app,
        "PUT",
        "/api/v1/me/tax-form",
        Some(&a.cookie),
        form("w9", "Ada", "US", "6789"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, got, _) = raw(
        &app,
        "GET",
        "/api/v1/me/tax-form",
        Some(&a.cookie),
        Value::Null,
    )
    .await;
    assert_eq!(got, body);
    assert_eq!(
        (got["kind"].as_str(), got["tax_id_last4"].as_str()),
        (Some("w9"), Some("6789"))
    );
    assert!(got["submitted_at"].is_string());
    // an admin has no affiliate profile
    let (st, ..) = raw(
        &app,
        "PUT",
        "/api/v1/me/tax-form",
        Some(&w.admin_cookie),
        form("w9", "A", "US", "1234"),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_report_1099_csv_escapes_formula_injection(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "w").await;
    let a = affiliate(&pool, &app, &w, "a@x.io").await;
    sale(&pool, &w, &a, "o1", 1_000_000).await; // $1000 commission
    raw(
        &app,
        "PUT",
        "/api/v1/me/tax-form",
        Some(&a.cookie),
        form("w9", "=1+1", "US", "1234"),
    )
    .await;
    pay(&pool, &w).await;
    let (st, csv, ctype) = text(&app, "/api/v1/reports/1099-style.csv", &w.admin_cookie).await;
    assert_eq!(st, StatusCode::OK);
    assert!(ctype.starts_with("text/csv"), "{ctype}");
    assert_eq!(
        csv,
        "legal_name,country,tax_form_kind,total_paid_cents,currency\n'=1+1,US,w9,100000,USD\n"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_report_1099_csv_only_includes_above_threshold(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "w").await;
    let (low, high, none) = (
        affiliate(&pool, &app, &w, "low@x.io").await,
        affiliate(&pool, &app, &w, "high@x.io").await,
        affiliate(&pool, &app, &w, "none@x.io").await,
    );
    sale(&pool, &w, &low, "o1", 500_000).await; // $500
    sale(&pool, &w, &high, "o2", 700_000).await; // $700
    sale(&pool, &w, &none, "o3", 600_000).await; // exactly $600: included (>=), no form
    raw(
        &app,
        "PUT",
        "/api/v1/me/tax-form",
        Some(&high.cookie),
        json!({"kind": "w8", "legal_name": "High Co", "country": "DE"}),
    )
    .await;
    pay(&pool, &w).await;
    // a paid-but-unpaid mix: owed but not yet paid does not count
    let late = affiliate(&pool, &app, &w, "late@x.io").await;
    sale(&pool, &w, &late, "o4", 900_000).await;
    approve_due(&pool).await.unwrap();

    let (st, csv, _) = text(&app, "/api/v1/reports/1099-style.csv", &w.admin_cookie).await;
    assert_eq!(st, StatusCode::OK);
    let mut lines: Vec<&str> = csv.lines().skip(1).collect();
    lines.sort_unstable();
    assert_eq!(
        lines,
        ["High Co,DE,w8,70000,USD", "none@x.io,,none,60000,USD"],
        "{csv}"
    );

    // wrong year: nothing was paid then
    let (_, csv, _) = text(
        &app,
        "/api/v1/reports/1099-style.csv?year=2001",
        &w.admin_cookie,
    )
    .await;
    assert_eq!(csv.lines().count(), 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_report_1099_csv_is_admin_and_workspace_scoped(pool: PgPool) {
    let app = app(pool.clone());
    let (w1, w2) = (
        workspace(&pool, &app, "w1").await,
        workspace(&pool, &app, "w2").await,
    );
    let a = affiliate(&pool, &app, &w1, "a@x.io").await;
    sale(&pool, &w1, &a, "o1", 1_000_000).await;
    pay(&pool, &w1).await;
    let (st, ..) = text(&app, "/api/v1/reports/1099-style.csv", &a.cookie).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, csv, _) = text(&app, "/api/v1/reports/1099-style.csv", &w2.admin_cookie).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(csv.lines().count(), 1, "other workspace leaked: {csv}");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_invoice_html_renders_line_items_matching_ledger(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "Acme & Sons").await;
    let (a, b) = (
        affiliate(&pool, &app, &w, "a@x.io").await,
        affiliate(&pool, &app, &w, "b@x.io").await,
    );
    sale(&pool, &w, &a, "o1", 12_345).await; // 1234.5 -> commission rounds per the engine
    sale(&pool, &w, &a, "o2", 50_000).await;
    sale(&pool, &w, &b, "o3", 99_900).await;
    let batch = pay(&pool, &w).await;
    let payout = payout_of(&pool, batch, a.id).await;
    let uri = format!("/api/v1/payouts/{payout}/invoice.html");

    let (st, html, ctype) = text(&app, &uri, &w.admin_cookie).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(ctype.starts_with("text/html"), "{ctype}");
    assert!(!html.contains("<script"), "invoice must not need JS");
    assert!(
        html.contains("Acme &amp; Sons"),
        "workspace name escaped: {html}"
    );
    assert!(html.contains(&payout.to_string()));
    assert!(
        html.contains("a@x.io"),
        "falls back to the email without a tax form"
    );
    assert!(!html.contains("b@x.io"));

    // The ledger, not payouts.amount_cents, is the reference.
    let lines: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT c.id, -COALESCE(sum(l.amount_cents), 0)::bigint FROM conversions c \
         LEFT JOIN ledger_entries l ON l.conversion_id = c.id AND l.account = 'aff_payable' \
         WHERE c.payout_id = $1 GROUP BY c.id",
    )
    .bind(payout)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(lines.len(), 2);
    for (id, commission) in &lines {
        assert!(html.contains(&id.to_string()), "line {id} missing");
        assert!(
            html.contains(&money(*commission)),
            "commission {commission} of {id} missing"
        );
    }
    let total: i64 = lines.iter().map(|l| l.1).sum();
    assert!(total > 0);
    assert!(
        html.contains(&format!("data-total=\"{total}\"")),
        "total {total} not in {html}"
    );
    assert!(html.contains(&format!("{} USD", money(total))));
    let aff_total: i64 = sqlx::query_scalar("SELECT amount_cents FROM payouts WHERE id = $1")
        .bind(payout)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(aff_total, total, "payout drifted from the ledger");
}

fn money(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_invoice_uses_legal_name_and_escapes_html(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "w").await;
    let a = affiliate(&pool, &app, &w, "a@x.io").await;
    sale(&pool, &w, &a, "o1", 100_000).await;
    raw(
        &app,
        "PUT",
        "/api/v1/me/tax-form",
        Some(&a.cookie),
        form("w9", "<script>alert(1)</script> \"Q\" & 'R'", "US", "1234"),
    )
    .await;
    let batch = pay(&pool, &w).await;
    let payout = payout_of(&pool, batch, a.id).await;
    let (st, html, _) = text(
        &app,
        &format!("/api/v1/payouts/{payout}/invoice.html"),
        &a.cookie,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!html.contains("<script"), "{html}");
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt; &quot;Q&quot; &amp; &#39;R&#39;"),
        "{html}"
    );
    assert!(!html.contains("1234"), "tax id must not be on the invoice");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_invoice_escapes_email_fallback(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "w").await;
    let a = affiliate(&pool, &app, &w, "\"><img src=x onerror=alert(1)>@x.io").await;
    sale(&pool, &w, &a, "o1", 100_000).await;
    let batch = pay(&pool, &w).await;
    let payout = payout_of(&pool, batch, a.id).await;
    let (st, html, _) = text(
        &app,
        &format!("/api/v1/payouts/{payout}/invoice.html"),
        &w.admin_cookie,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!html.contains("<img"), "{html}");
    assert!(
        html.contains("&lt;img src=x onerror=alert(1)&gt;@x.io"),
        "{html}"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_invoice_access_is_owner_or_workspace_admin(pool: PgPool) {
    let app = app(pool.clone());
    let (w1, w2) = (
        workspace(&pool, &app, "w1").await,
        workspace(&pool, &app, "w2").await,
    );
    let (a, b) = (
        affiliate(&pool, &app, &w1, "a@x.io").await,
        affiliate(&pool, &app, &w1, "b@x.io").await,
    );
    sale(&pool, &w1, &a, "o1", 100_000).await;
    sale(&pool, &w1, &b, "o2", 100_000).await;
    let batch = pay(&pool, &w1).await;
    let payout = payout_of(&pool, batch, a.id).await;
    let uri = format!("/api/v1/payouts/{payout}/invoice.html");
    assert_eq!(text(&app, &uri, &a.cookie).await.0, StatusCode::OK);
    assert_eq!(text(&app, &uri, &w1.admin_cookie).await.0, StatusCode::OK);
    assert_eq!(text(&app, &uri, &b.cookie).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        text(&app, &uri, &w2.admin_cookie).await.0,
        StatusCode::NOT_FOUND
    );
    let missing = format!("/api/v1/payouts/{}/invoice.html", Uuid::now_v7());
    assert_eq!(
        text(&app, &missing, &w1.admin_cookie).await.0,
        StatusCode::NOT_FOUND
    );
    let (st, ..) = raw_bytes(&app, "GET", &uri, None, Value::Null).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tax_invoices_draft_payout_has_no_invoice_for_affiliate(pool: PgPool) {
    let app = app(pool.clone());
    let w = workspace(&pool, &app, "w").await;
    let a = affiliate(&pool, &app, &w, "a@x.io").await;
    sale(&pool, &w, &a, "o1", 100_000).await;
    approve_due(&pool).await.unwrap();
    let batch = create_batch(&pool, w.id, w.program, w.admin)
        .await
        .unwrap()
        .unwrap();
    let payout = payout_of(&pool, batch, a.id).await;
    let uri = format!("/api/v1/payouts/{payout}/invoice.html");
    assert_eq!(text(&app, &uri, &a.cookie).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        text(&app, &uri, &w.admin_cookie).await.0,
        StatusCode::NOT_FOUND
    );
}
