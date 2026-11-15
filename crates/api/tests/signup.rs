use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{Request, Response, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PW: &str = "long enough pw";

fn app(pool: PgPool) -> Router {
    // SAFETY: set before any request; every test in this binary wants signup on.
    unsafe { std::env::set_var("SIGNUP_ENABLED", "1") };
    let ring = KeyRing::from_pairs("t1", &[("t1", [7u8; 32])]).unwrap();
    let state = AppState {
        pool,
        jwt_keys: api::auth::JwtKeys::single("test-secret").into(),
        version_cache: Default::default(),
        tracker_url: Default::default(),
        vault: std::sync::Arc::new(Vault::new(ring)),
        secure_cookies: true,
        limiter: Default::default(),
    };
    build(routes::routes(), state)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    ip: &str,
    body: Value,
) -> Response<Body> {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(HOST, "bf.test")
        .header(ORIGIN, "https://bf.test")
        .header(COOKIE, format!("{cookie}; bf_csrf=t"))
        .header("x-csrf-token", "t")
        .header("x-forwarded-for", ip)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn signup(app: &Router, name: &str, email: &str, pw: &str, ip: &str) -> Response<Body> {
    send(
        app,
        "POST",
        "/api/v1/signup",
        "",
        ip,
        json!({"workspace_name": name, "admin_email": email, "admin_password": pw}),
    )
    .await
}

async fn body(res: Response<Body>) -> Value {
    let b = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&b).unwrap_or(Value::Null)
}

fn cookies(res: &Response<Body>) -> String {
    res.headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter(|c| c.starts_with("bf_at="))
        .map(|c| c.split(';').next().unwrap().to_string())
        .next()
        .expect("bf_at cookie")
}

async fn ws_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workspaces")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_creates_workspace_and_logs_in(pool: PgPool) {
    let app = app(pool.clone());
    let res = signup(&app, "Acme", "a@x.io", PW, "1.1.1.1").await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let cookie = cookies(&res);
    let b = body(res).await;
    assert_eq!(b["role"], "admin");
    let ws = b["workspace_id"].as_str().unwrap().to_string();
    let me = send(
        &app,
        "GET",
        "/api/v1/auth/me",
        &cookie,
        "1.1.1.1",
        json!(null),
    )
    .await;
    assert_eq!(me.status(), StatusCode::OK);
    let me = body(me).await;
    assert_eq!(
        (me["workspace_id"].as_str().unwrap(), &me["role"]),
        (ws.as_str(), &json!("admin"))
    );
    let ws_id: Uuid = ws.parse().unwrap();
    let via: String = sqlx::query_scalar("SELECT created_via FROM workspaces WHERE id = $1")
        .bind(ws_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(via, "self_serve");
    // default plan subscription from B09
    let subs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscriptions WHERE workspace_id = $1")
            .bind(ws_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(subs, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_rejects_bad_input(pool: PgPool) {
    let app = app(pool.clone());
    for (name, email, pw) in [
        ("Acme", "a@x.io", "short"),
        ("Acme", "nope", PW),
        ("  ", "a@x.io", PW),
    ] {
        let res = signup(&app, name, email, pw, "1.1.1.2").await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{name} {email} {pw}");
    }
    assert_eq!(ws_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_slugifies_and_dedupes(pool: PgPool) {
    let app = app(pool.clone());
    for email in ["a@x.io", "b@x.io"] {
        assert_eq!(
            signup(&app, "Acme", email, PW, "1.1.1.3").await.status(),
            StatusCode::CREATED
        );
    }
    let slugs: Vec<String> = sqlx::query_scalar("SELECT slug FROM workspaces ORDER BY slug")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(slugs, ["acme", "acme-2"]);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_duplicate_email_conflicts_without_orphan(pool: PgPool) {
    let app = app(pool.clone());
    assert_eq!(
        signup(&app, "Acme", "a@x.io", PW, "1.1.1.4").await.status(),
        StatusCode::CREATED
    );
    let res = signup(&app, "Other", "A@X.io", PW, "1.1.1.4").await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(ws_count(&pool).await, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_rate_limited(pool: PgPool) {
    let app = app(pool);
    let mut last = StatusCode::OK;
    for i in 0..12 {
        last = signup(&app, "Acme", &format!("u{i}@x.io"), "short", "9.9.9.9")
            .await
            .status();
        if last == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_workspaces_are_isolated(pool: PgPool) {
    let app = app(pool.clone());
    let a = signup(&app, "Alpha", "a@x.io", PW, "2.2.2.1").await;
    let ca = cookies(&a);
    let wa = body(a).await["workspace_id"].as_str().unwrap().to_string();
    let b = signup(&app, "Beta", "b@x.io", PW, "2.2.2.2").await;
    let cb = cookies(&b);
    let wb = body(b).await["workspace_id"].as_str().unwrap().to_string();
    assert_ne!(wa, wb);

    // Beta's admin writes a contact request into Beta; Alpha's admin must not see it.
    let msg = json!({"workspace": "beta", "name": "N", "email": "n@x.io", "message": "hi"});
    assert_eq!(
        send(&app, "POST", "/api/v1/contact-requests", "", "2.2.2.3", msg)
            .await
            .status(),
        StatusCode::CREATED
    );
    let la = body(
        send(
            &app,
            "GET",
            "/api/v1/contact-requests",
            &ca,
            "2.2.2.1",
            json!(null),
        )
        .await,
    )
    .await;
    let lb = body(
        send(
            &app,
            "GET",
            "/api/v1/contact-requests",
            &cb,
            "2.2.2.2",
            json!(null),
        )
        .await,
    )
    .await;
    assert_eq!(
        (la["total"].as_i64(), lb["total"].as_i64()),
        (Some(0), Some(1))
    );

    // A self-serve admin is workspace admin only: no super-admin routes, no foreign workspace data.
    let ws = send(
        &app,
        "GET",
        "/api/v1/workspaces",
        &ca,
        "2.2.2.1",
        json!(null),
    )
    .await;
    assert_eq!(ws.status(), StatusCode::FORBIDDEN);
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'super_admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 0);
}
