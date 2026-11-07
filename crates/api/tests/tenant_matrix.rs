//! Cross-tenant route matrix (C02). Two workspaces, A and B. Every annotated API route that is
//! not public is called as A with B's ids; nothing of B may be reachable, readable or changed.
//! Known real leaks live in `known_leaks.txt` (xfail list, emptied by C04).
//! Run: `cargo test -p api --test tenant_matrix -- --test-threads=1`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use api::auth::{Principal, issue_access};
use api::worker::process_events;
use api::{AppState, app};
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{Method, Request, StatusCode};
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use bf_db::tracker::{InboundEvent, insert_events};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-secret";
const MARK: &str = "B-SECRET-NAME";

struct Api(axum::Router);

impl Api {
    fn new(pool: &PgPool) -> Self {
        let state = AppState {
            pool: pool.clone(),
            jwt_keys: api::auth::JwtKeys::single(SECRET).into(),
            version_cache: Default::default(),
            vault: std::sync::Arc::new(bf_core::secrets::Vault::plaintext()),
            secure_cookies: true,
            limiter: Default::default(),
        };
        Self(app(state))
    }

    async fn call(
        &self,
        m: &str,
        uri: &str,
        cookie: Option<&str>,
        body: Option<&Value>,
    ) -> (StatusCode, String) {
        let method: Method = m.to_uppercase().parse().unwrap();
        let mut b = Request::builder()
            .method(method.clone())
            .uri(uri)
            .header(HOST, "bf.test");
        if method != Method::GET {
            b = b
                .header(ORIGIN, "https://bf.test")
                .header("x-csrf-token", "t");
        }
        // Unauthenticated writes still carry the CSRF pair, so the auth layer answers, not CSRF.
        let csrf = if method != Method::GET {
            "bf_csrf=t"
        } else {
            ""
        };
        let jar = match cookie {
            Some(c) if csrf.is_empty() => c.to_string(),
            Some(c) => format!("{c}; {csrf}"),
            None => csrf.to_string(),
        };
        if !jar.is_empty() {
            b = b.header(COOKIE, jar);
        }
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
        (st, String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[derive(Clone)]
struct Ws {
    ws: Uuid,
    admin: String,
    aff_cookie: String,
    program: Uuid,
    plan: Uuid,
    ctype: Uuid,
    domain: String,
    affiliate: Uuid,
    code: String,
    customer: Uuid,
    conversion: Uuid,
    batch: Uuid,
    endpoint: Uuid,
    delivery: Uuid,
    template: Uuid,
    notification: Uuid,
    contact: Uuid,
    #[allow(dead_code)]
    admin_user: Uuid,
}

async fn user(pool: &PgPool, ws: Uuid, role: UserRole, email: &str) -> (Uuid, String) {
    let u = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: email.into(),
        password_hash: "unused".into(),
        role,
    };
    create_user(pool, &u).await.unwrap();
    let token = issue_access(SECRET, &Principal::from(&u)).unwrap();
    (u.id, format!("bf_at={token}"))
}

async fn exec(pool: &PgPool, q: &str, ids: &[Uuid]) {
    let mut q = sqlx::query(q);
    for i in ids {
        q = q.bind(*i);
    }
    q.execute(pool).await.unwrap();
}

/// Seed one workspace with one of everything. `tag` is "a" or "b"; B's names carry [`MARK`].
async fn seed(pool: &PgPool, tag: &str) -> Ws {
    let secret = if tag == "b" { MARK } else { "a-name" };
    let ws = Uuid::now_v7();
    create_workspace(
        pool,
        ws,
        &format!("ws-{tag}-{ws}"),
        &format!("{secret}-{ws}"),
    )
    .await
    .unwrap();
    let (admin_user, admin) =
        user(pool, ws, UserRole::Admin, &format!("admin-{tag}-{ws}@x.io")).await;
    let (aff_user, aff_cookie) = user(
        pool,
        ws,
        UserRole::Affiliate,
        &format!("aff-{tag}-{ws}@x.io"),
    )
    .await;
    let (program, plan, ctype, affiliate) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let code = format!("{}-{tag}-CODE", ws.simple());
    let domain = format!("{tag}-{}.example.com", ws.simple());
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, auto_approve, hold_days, public_key, webhook_secret) \
         VALUES ($1, $2, $3, 'USD', false, 0, $4, 's')",
    )
    .bind(program).bind(ws).bind(format!("{secret}-program")).bind(program.to_string())
    .execute(pool).await.unwrap();
    exec(pool, "INSERT INTO commission_plans (id, program_id, kind, rate_bps, duration) VALUES ($1, $2, 'percent', 1000, 'lifetime')", &[plan, program]).await;
    exec(
        pool,
        "UPDATE programs SET default_plan_id = $2 WHERE id = $1",
        &[program, plan],
    )
    .await;
    sqlx::query(
        "INSERT INTO conversion_types (id, program_id, key, name) VALUES ($1, $2, 'signup', $3)",
    )
    .bind(ctype)
    .bind(program)
    .bind(format!("{secret}-type"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO program_sites (program_id, domain) VALUES ($1, $2)")
        .bind(program)
        .bind(&domain)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO affiliates (id, workspace_id, user_id, status, payout_email) VALUES ($1, $2, $3, 'active', $4)")
        .bind(affiliate).bind(ws).bind(aff_user).bind(format!("{secret}@pay.io")).execute(pool).await.unwrap();
    exec(
        pool,
        "INSERT INTO enrollments (affiliate_id, program_id) VALUES ($1, $2)",
        &[affiliate, program],
    )
    .await;
    sqlx::query("INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) VALUES ($1, $2, $3, $4)")
        .bind(&code).bind(affiliate).bind(program).bind(format!("https://{domain}/")).execute(pool).await.unwrap();

    // customer, conversion and ledger entries via the real ingest path
    let ext = format!("order-{tag}-{ws}");
    insert_events(
        pool,
        &[InboundEvent {
            id: Uuid::now_v7(),
            program_id: program,
            source: "generic".into(),
            external_id: ext.clone(),
            payload: json!({"customer": ext, "amount_cents": 10_000, "code": code,
                        "occurred_at": (Utc::now() - Duration::hours(1)).to_rfc3339()}),
            received_ms: Utc::now().timestamp_millis(),
        }],
    )
    .await
    .unwrap();
    process_events(pool).await.unwrap();
    let conversion: Uuid = sqlx::query_scalar("SELECT id FROM conversions WHERE program_id = $1")
        .bind(program)
        .fetch_one(pool)
        .await
        .unwrap();
    let customer: Uuid = sqlx::query_scalar("SELECT customer_id FROM conversions WHERE id = $1")
        .bind(conversion)
        .fetch_one(pool)
        .await
        .unwrap();

    let batch = Uuid::now_v7();
    exec(
        pool,
        "INSERT INTO payout_batches (id, program_id, created_by) VALUES ($1, $2, $3)",
        &[batch, program, admin_user],
    )
    .await;
    let (endpoint, delivery, template, notification, contact) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query(
        "INSERT INTO webhook_endpoints (id, workspace_id, url, secret) VALUES ($1, $2, $3, 'sec')",
    )
    .bind(endpoint)
    .bind(ws)
    .bind(format!("https://{secret}.example.com/hook"))
    .execute(pool)
    .await
    .unwrap();
    let outbox: i64 =
        sqlx::query_scalar("INSERT INTO outbox (topic, payload) VALUES ('t', $1) RETURNING id")
            .bind(json!({"note": secret}))
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO webhook_deliveries (id, endpoint_id, outbox_id, status_code) VALUES ($1, $2, $3, 500)")
        .bind(delivery).bind(endpoint).bind(outbox).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO email_templates (id, workspace_id, key, subject, body) VALUES ($1, $2, 'welcome', $3, 'body')")
        .bind(template).bind(ws).bind(format!("{secret}-subject")).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO notifications (id, user_id, kind, body) VALUES ($1, $2, 'k', $3)")
        .bind(notification)
        .bind(admin_user)
        .bind(format!("{secret}-note"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO contact_requests (id, workspace_id, program_id, name, email, message) VALUES ($1, $2, $3, $4, 'c@x.io', 'm')")
        .bind(contact).bind(ws).bind(program).bind(format!("{secret}-contact")).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO audit_log (id, workspace_id, actor_id, action, entity, entity_id) VALUES ($1, $2, $3, 'seed', 'program', $4)")
        .bind(Uuid::now_v7()).bind(ws).bind(admin_user).bind(format!("{secret}-audit")).execute(pool).await.unwrap();

    Ws {
        ws,
        admin,
        aff_cookie,
        program,
        plan,
        ctype,
        domain,
        affiliate,
        code,
        customer,
        conversion,
        batch,
        endpoint,
        delivery,
        template,
        notification,
        contact,
        admin_user,
    }
}

// ---- annotations / route table ----

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Clone, Debug)]
struct Ann {
    method: String,
    path: String,
    kind: String,
    role: String,
}

fn annotations() -> Vec<Ann> {
    let text = std::fs::read_to_string(root().join("docs/audit/tenant-annotations.toml")).unwrap();
    let (mut out, mut cur): (Vec<Ann>, Option<Ann>) = (vec![], None);
    for line in text.lines().map(str::trim) {
        if line == "[[route]]" {
            out.extend(cur.take());
            cur = Some(Ann {
                method: String::new(),
                path: String::new(),
                kind: String::new(),
                role: String::new(),
            });
        } else if let (Some(a), Some((k, v))) = (cur.as_mut(), line.split_once(" = ")) {
            let v = v.trim().trim_matches('"').to_owned();
            match k {
                "method" => a.method = v,
                "path" => a.path = v,
                "kind" => a.kind = v,
                "role" => a.role = v,
                _ => {}
            }
        }
    }
    out.extend(cur);
    out
}

/// API routes (from `route_table()`) joined with their annotation. Tracker routes are excluded:
/// they are not tenant-session routes.
fn api_routes() -> Vec<(Ann, Vec<String>)> {
    let anns = annotations();
    api::routes::route_table()
        .into_iter()
        .map(|r| {
            let a = anns
                .iter()
                .find(|a| a.method == r.method && a.path == r.path)
                .unwrap_or_else(|| panic!("no annotation for {} {}", r.method, r.path))
                .clone();
            (a, r.path_params)
        })
        .collect()
}

fn key(a: &Ann) -> String {
    format!("{} {}", a.method.to_uppercase(), a.path)
}

fn known_leaks() -> BTreeSet<String> {
    let text = std::fs::read_to_string(root().join("crates/api/tests/known_leaks.txt"))
        .unwrap_or_default();
    text.lines()
        .map(|l| l.split('#').next().unwrap().trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Replace every `{param}` with the matching resource id of `ws`.
fn fill(path: &str, ids: &Ws) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let j = rest.find('}').unwrap();
        let name = &rest[i + 1..j];
        let prefix = &path[..path.find(&rest[i..]).unwrap_or(0)];
        let v = match name {
            "id" => resource_for(prefix, ids),
            "code" => ids.code.clone(),
            "domain" => ids.domain.clone(),
            "affiliate_id" => ids.affiliate.to_string(),
            "plan_id" => ids.plan.to_string(),
            "type_id" => ids.ctype.to_string(),
            other => panic!("unmapped path param {other}"),
        };
        out.push_str(&v);
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    out
}

fn resource_for(prefix: &str, ids: &Ws) -> String {
    let id = |u: Uuid| u.to_string();
    let tbl = [
        ("/api/v1/affiliates", ids.affiliate),
        ("/api/v1/contact-requests", ids.contact),
        ("/api/v1/conversions", ids.conversion),
        ("/api/v1/customers", ids.customer),
        ("/api/v1/email-templates", ids.template),
        ("/api/v1/notifications", ids.notification),
        ("/api/v1/payouts/batches", ids.batch),
        ("/api/v1/programs", ids.program),
        ("/api/v1/webhooks/deliveries", ids.delivery),
        ("/api/v1/webhooks/endpoints", ids.endpoint),
    ];
    tbl.iter()
        .filter(|(p, _)| prefix.starts_with(p))
        .max_by_key(|(p, _)| p.len())
        .map(|(_, u)| id(*u))
        .unwrap_or_else(|| panic!("no resource for {prefix}"))
}

/// Body for writes, keyed by `(method, path)`, referencing the foreign workspace's ids.
fn body(m: &str, path: &str, f: &Ws) -> Option<Value> {
    Some(match (m, path) {
        ("patch", "/api/v1/affiliates/{id}") | ("patch", "/api/v1/me/affiliate") => {
            json!({"payout_email": "hijack@x.io"})
        }
        ("patch", "/api/v1/contact-requests/{id}") => json!({"status": "closed"}),
        ("post", "/api/v1/email-templates") => {
            json!({"key": "welcome", "subject": "s", "body": "b"})
        }
        ("patch", "/api/v1/email-templates/{id}") => json!({"subject": "hijacked"}),
        ("post", "/api/v1/me/referral-codes") => {
            json!({"program_id": f.program, "code": "XTENANT1", "landing_url": format!("https://{}/", f.domain)})
        }
        ("post", "/api/v1/members") => {
            json!({"email": "intruder@x.io", "password": "correct horse battery"})
        }
        ("post", "/api/v1/payouts/batches") => json!({"program_id": f.program}),
        ("patch", "/api/v1/programs/{id}") => json!({"name": "hijacked"}),
        ("post", "/api/v1/programs/{id}/conversion-types") => json!({"key": "x", "name": "x"}),
        ("post", "/api/v1/programs/{id}/enrollments") => json!({"affiliate_id": f.affiliate}),
        ("post", "/api/v1/programs/{id}/plans") => {
            json!({"kind": "percent", "rate_bps": 5000, "duration": "lifetime"})
        }
        ("post", "/api/v1/programs/{id}/sites") => json!({"domain": "evil.example.com"}),
        ("post", "/api/v1/referral-codes") => {
            json!({"affiliate_id": f.affiliate, "program_id": f.program, "code": "XTENANT2"})
        }
        ("put", "/api/v1/settings") => json!({}),
        ("post", "/api/v1/webhooks/endpoints") => json!({"url": "https://evil.example.com/h"}),
        ("patch", "/api/v1/webhooks/endpoints/{id}") => json!({"active": false}),
        ("post", "/api/v1/programs") => json!({"name": "x", "currency": "USD"}),
        ("post", "/api/v1/workspaces") => json!({"name": "x"}),
        ("post", "/api/v1/auth/totp/confirm")
        | ("post", "/api/v1/auth/totp/disable")
        | ("post", "/api/v1/auth/totp/recovery-codes/regenerate") => {
            json!({"current_password": "x", "code": "000000"})
        }
        ("post", "/api/v1/auth/totp/enrol") => json!({}),
        _ => return None,
    })
}

/// Query string that points a list/read at the foreign workspace's ids.
fn query(m: &str, path: &str, f: &Ws) -> String {
    if m != "get" {
        return String::new();
    }
    match path {
        "/api/v1/ledger/entries"
        | "/api/v1/ledger/balances"
        | "/api/v1/analytics/summary"
        | "/api/v1/analytics/timeseries"
        | "/api/v1/reports/analytics.csv"
        | "/api/v1/conversions"
        | "/api/v1/customers"
        | "/api/v1/payouts/batches"
        | "/api/v1/referral-codes" => {
            format!("?program_id={}&affiliate_id={}", f.program, f.affiliate)
        }
        "/api/v1/webhooks/deliveries" => format!("?endpoint_id={}", f.endpoint),
        "/api/v1/members" => format!("?workspace_id={}", f.ws),
        _ => String::new(),
    }
}

// ---- B snapshot ----

const SNAP: [(&str, &str); 17] = [
    (
        "workspaces",
        "SELECT id::text, name, slug FROM workspaces WHERE id = $1",
    ),
    (
        "users",
        "SELECT id::text, email, role::text, workspace_id::text FROM users WHERE workspace_id = $1",
    ),
    ("programs", "SELECT * FROM programs WHERE workspace_id = $1"),
    (
        "plans",
        "SELECT c.* FROM commission_plans c JOIN programs p ON p.id = c.program_id WHERE p.workspace_id = $1",
    ),
    (
        "types",
        "SELECT c.* FROM conversion_types c JOIN programs p ON p.id = c.program_id WHERE p.workspace_id = $1",
    ),
    (
        "sites",
        "SELECT c.* FROM program_sites c JOIN programs p ON p.id = c.program_id WHERE p.workspace_id = $1",
    ),
    (
        "affiliates",
        "SELECT * FROM affiliates WHERE workspace_id = $1",
    ),
    (
        "enrollments",
        "SELECT e.* FROM enrollments e JOIN affiliates a ON a.id = e.affiliate_id WHERE a.workspace_id = $1",
    ),
    (
        "codes",
        "SELECT c.* FROM referral_codes c JOIN programs p ON p.id = c.program_id WHERE p.workspace_id = $1",
    ),
    (
        "conversions",
        "SELECT c.* FROM conversions c JOIN programs p ON p.id = c.program_id WHERE p.workspace_id = $1",
    ),
    (
        "ledger",
        "SELECT c.* FROM ledger_entries c JOIN programs p ON p.id = c.program_id WHERE p.workspace_id = $1",
    ),
    (
        "batches",
        "SELECT c.* FROM payout_batches c JOIN programs p ON p.id = c.program_id WHERE p.workspace_id = $1",
    ),
    (
        "endpoints",
        "SELECT * FROM webhook_endpoints WHERE workspace_id = $1",
    ),
    (
        "deliveries",
        "SELECT d.* FROM webhook_deliveries d JOIN webhook_endpoints e ON e.id = d.endpoint_id WHERE e.workspace_id = $1",
    ),
    (
        "templates",
        "SELECT * FROM email_templates WHERE workspace_id = $1",
    ),
    (
        "notifications",
        "SELECT n.* FROM notifications n JOIN users u ON u.id = n.user_id WHERE u.workspace_id = $1",
    ),
    (
        "contacts",
        "SELECT * FROM contact_requests WHERE workspace_id = $1",
    ),
];

/// One text blob of all B rows (row-to-text, ordered), so any change is visible.
async fn snapshot(pool: &PgPool, ws: Uuid) -> String {
    let mut out = String::new();
    for (name, q) in SNAP {
        let wrapped =
            format!("SELECT coalesce(string_agg(t::text, '|' ORDER BY t::text), '') FROM ({q}) t");
        let s: String = sqlx::query_scalar(&wrapped)
            .bind(ws)
            .fetch_one(pool)
            .await
            .unwrap();
        out.push_str(&format!("{name}:{s}\n"));
    }
    out
}

async fn world(pool: &PgPool) -> (Api, Ws, Ws) {
    (Api::new(pool), seed(pool, "a").await, seed(pool, "b").await)
}

fn is_denied(st: StatusCode) -> bool {
    st == StatusCode::NOT_FOUND || st == StatusCode::FORBIDDEN
}

/// Non-public routes A may call with B ids: scoped + any `self` route that takes an id.
fn cross_routes() -> Vec<(Ann, Vec<String>)> {
    api_routes()
        .into_iter()
        .filter(|(a, p)| a.kind == "scoped" || (a.kind == "self" && !p.is_empty()))
        .collect()
}

struct Run {
    executed: usize,
    violations: Vec<(String, String)>,
}

async fn run_matrix(pool: &PgPool, check_snapshot: bool) -> Run {
    let (api, a, b) = world(pool).await;
    let mut run = Run {
        executed: 0,
        violations: vec![],
    };
    for (ann, _) in cross_routes() {
        // `/me/*` and affiliate-role routes are called as A's affiliate, the rest as A's admin.
        let cookie = if ann.role == "Affiliate" {
            &a.aff_cookie
        } else {
            &a.admin
        };
        let uri = format!(
            "{}{}",
            fill(&ann.path, &b),
            query(&ann.method, &ann.path, &b)
        );
        let req_body = body(&ann.method, &ann.path, &b);
        let before = if check_snapshot {
            Some(snapshot(pool, b.ws).await)
        } else {
            None
        };
        let (st, text) = api
            .call(&ann.method, &uri, Some(cookie), req_body.as_ref())
            .await;
        run.executed += 1;
        let k = key(&ann);
        if std::env::var("PRINT_LEAKS").is_ok() {
            println!("STATUS {st} {k} {uri}");
        }
        let mut why = vec![];
        // A list endpoint with no id param answers 200 with A's own rows; every other call
        // must be denied.
        let no_id = !ann.path.contains('{');
        if !no_id && !is_denied(st) {
            why.push(format!("status {st}"));
        }
        if no_id
            && !(st.is_success()
                || is_denied(st)
                || st == StatusCode::BAD_REQUEST
                || st == StatusCode::UNPROCESSABLE_ENTITY)
        {
            why.push(format!("status {st}"));
        }
        if st.is_server_error() {
            why.push(format!("5xx {st}"));
        }
        if text.contains(MARK) {
            why.push("body contains B marker".into());
        }
        let foreign = [
            b.delivery,
            b.conversion,
            b.batch,
            b.customer,
            b.affiliate,
            b.template,
            b.contact,
            b.endpoint,
        ];
        if !is_denied(st) && foreign.iter().any(|id| text.contains(&id.to_string())) {
            why.push("body contains a B row id".into());
        }
        if text.contains(&b.ws.to_string()) && !is_denied(st) {
            why.push("body contains B workspace id".into());
        }
        if let Some(before) = before {
            if snapshot(pool, b.ws).await != before {
                why.push("B rows changed".into());
            }
        }
        if !why.is_empty() {
            run.violations.push((k, why.join("; ")));
        }
    }
    run
}

fn unexpected(run: &Run) -> Vec<String> {
    let known = known_leaks();
    run.violations
        .iter()
        .filter(|(k, _)| !known.contains(k))
        .map(|(k, w)| format!("{k}: {w}"))
        .collect()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn matrix_covers_every_scoped_route(pool: PgPool) {
    let run = run_matrix(&pool, false).await;
    assert_eq!(run.executed, cross_routes().len());
    let scoped = api_routes()
        .iter()
        .filter(|(a, _)| a.kind == "scoped")
        .count();
    assert!(run.executed >= scoped, "matrix skipped scoped routes");
    if std::env::var("PRINT_LEAKS").is_ok() {
        for (k, w) in &run.violations {
            println!("LEAK {k} # {w}");
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn matrix_cross_tenant_read_denied(pool: PgPool) {
    let run = run_matrix(&pool, false).await;
    let bad: Vec<_> = unexpected(&run)
        .into_iter()
        .filter(|l| l.starts_with("GET "))
        .collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn matrix_cross_tenant_write_denied_and_no_side_effect(pool: PgPool) {
    let run = run_matrix(&pool, true).await;
    let bad: Vec<_> = unexpected(&run)
        .into_iter()
        .filter(|l| !l.starts_with("GET "))
        .collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn lists_never_contain_foreign_rows(pool: PgPool) {
    let (api, a, b) = world(&pool).await;
    let mut checked = 0;
    for (ann, _) in api_routes().into_iter().filter(|(a, p)| {
        a.method == "get" && p.is_empty() && (a.kind == "scoped" || a.kind == "self")
    }) {
        let cookie = if ann.role == "Affiliate" {
            &a.aff_cookie
        } else {
            &a.admin
        };
        let (st, text) = api.call("get", &ann.path, Some(cookie), None).await;
        checked += 1;
        if known_leaks().contains(&key(&ann)) {
            continue;
        }
        assert!(st.is_success(), "{} -> {st}", key(&ann));
        assert!(!text.contains(MARK), "{} leaks B rows", key(&ann));
        for id in [
            b.program,
            b.affiliate,
            b.conversion,
            b.batch,
            b.endpoint,
            b.template,
            b.contact,
        ] {
            assert!(
                !text.contains(&id.to_string()),
                "{} leaks B id {id}",
                key(&ann)
            );
        }
    }
    assert!(checked > 10);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn workspace_id_query_param_forbidden_for_non_super(pool: PgPool) {
    let (api, a, b) = world(&pool).await;
    let (st, text) = api
        .call(
            "get",
            &format!("/api/v1/members?workspace_id={}", b.ws),
            Some(&a.admin),
            None,
        )
        .await;
    let leaked = text.contains(MARK) || text.contains(&format!("admin-b-{}", b.ws));
    let key = "GET /api/v1/members";
    if known_leaks().contains(key) {
        return;
    }
    assert!(st == StatusCode::FORBIDDEN && !leaked, "{st} {text}");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn super_admin_can_scope_to_other_workspace(pool: PgPool) {
    let (api, _a, b) = world(&pool).await;
    let (_, sup) = user(&pool, b.ws, UserRole::SuperAdmin, "super@x.io").await;
    let (st, text) = api
        .call(
            "get",
            &format!("/api/v1/members?workspace_id={}", b.ws),
            Some(&sup),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    let rows = v["items"].as_array().or(v.as_array()).expect("list body");
    assert!(rows.iter().any(|r| {
        r["email"]
            .as_str()
            .is_some_and(|e| e.contains(&format!("admin-b-{}", b.ws)))
    }));
    assert!(rows.iter().all(|r| {
        r["workspace_id"]
            .as_str()
            .is_none_or(|w| w == b.ws.to_string())
    }));
}

// ---- role escalation ----

#[sqlx::test(migrations = "../db/migrations")]
async fn role_unauthenticated_gets_401_on_every_non_public_route(pool: PgPool) {
    let (api, a, _b) = world(&pool).await;
    let mut n = 0;
    for (ann, _) in api_routes().into_iter().filter(|(a, _)| a.kind != "public") {
        let uri = format!(
            "{}{}",
            fill(&ann.path, &a),
            query(&ann.method, &ann.path, &a)
        );
        let (st, _) = api
            .call(
                &ann.method,
                &uri,
                None,
                body(&ann.method, &ann.path, &a).as_ref(),
            )
            .await;
        n += 1;
        if known_leaks().contains(&format!("{} [unauth]", key(&ann))) {
            continue;
        }
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{}", key(&ann));
    }
    assert!(n > 50);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn role_affiliate_denied_on_admin_and_super_routes(pool: PgPool) {
    let (api, a, _b) = world(&pool).await;
    let mut n = 0;
    for (ann, _) in api_routes()
        .into_iter()
        .filter(|(a, _)| (a.kind == "scoped" || a.kind == "super") && a.role != "Any")
    {
        let uri = format!(
            "{}{}",
            fill(&ann.path, &a),
            query(&ann.method, &ann.path, &a)
        );
        let (st, _) = api
            .call(
                &ann.method,
                &uri,
                Some(&a.aff_cookie),
                body(&ann.method, &ann.path, &a).as_ref(),
            )
            .await;
        n += 1;
        if known_leaks().contains(&format!("{} [affiliate]", key(&ann))) {
            continue;
        }
        assert_eq!(st, StatusCode::FORBIDDEN, "{}", key(&ann));
    }
    assert!(n > 50);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn role_admin_denied_on_super_and_affiliate_routes(pool: PgPool) {
    let (api, a, _b) = world(&pool).await;
    for (ann, _) in api_routes()
        .into_iter()
        .filter(|(a, _)| a.kind == "super" || a.role == "Affiliate")
    {
        let uri = format!(
            "{}{}",
            fill(&ann.path, &a),
            query(&ann.method, &ann.path, &a)
        );
        let (st, _) = api
            .call(
                &ann.method,
                &uri,
                Some(&a.admin),
                body(&ann.method, &ann.path, &a).as_ref(),
            )
            .await;
        if known_leaks().contains(&format!("{} [admin]", key(&ann))) {
            continue;
        }
        assert_eq!(st, StatusCode::FORBIDDEN, "{}", key(&ann));
    }
}

// ---- random ids ----

#[sqlx::test(migrations = "../db/migrations")]
async fn random_id_param_never_200_or_500(pool: PgPool) {
    let (api, a, _b) = world(&pool).await;
    for (ann, params) in cross_routes().into_iter().filter(|(_, p)| !p.is_empty()) {
        let cookie = if ann.role == "Affiliate" {
            &a.aff_cookie
        } else {
            &a.admin
        };
        let mut uri = ann.path.clone();
        for p in &params {
            let v = if p == "code" {
                "NOSUCHCODE".to_string()
            } else if p == "domain" {
                "nosuch.example.com".to_string()
            } else {
                Uuid::now_v7().to_string()
            };
            uri = uri.replace(&format!("{{{p}}}"), &v);
        }
        let (st, _) = api
            .call(
                &ann.method,
                &uri,
                Some(cookie),
                body(&ann.method, &ann.path, &a).as_ref(),
            )
            .await;
        if known_leaks().contains(&format!("{} [random]", key(&ann))) {
            continue;
        }
        assert!(!st.is_server_error(), "{} -> {st}", key(&ann));
        // creating under an unknown parent must not succeed either
        assert!(
            is_denied(st)
                || st == StatusCode::BAD_REQUEST
                || st == StatusCode::UNPROCESSABLE_ENTITY,
            "{} -> {st}",
            key(&ann)
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn affiliate_cannot_mint_code_under_foreign_program(pool: PgPool) {
    let (api, a, b) = world(&pool).await;
    let before = snapshot(&pool, b.ws).await;
    let body = json!({"program_id": b.program, "code": "XTENANT3"});
    let (st, text) = api
        .call(
            "post",
            "/api/v1/me/referral-codes",
            Some(&a.aff_cookie),
            Some(&body),
        )
        .await;
    if !known_leaks().contains("POST /api/v1/me/referral-codes") {
        assert!(is_denied(st), "{st} {text}");
    }
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM referral_codes WHERE code = 'XTENANT3' AND program_id = $1",
    )
    .bind(b.program)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(n == 0 || known_leaks().contains("POST /api/v1/me/referral-codes"));
    assert!(
        snapshot(&pool, b.ws).await == before
            || known_leaks().contains("POST /api/v1/me/referral-codes")
    );
}
