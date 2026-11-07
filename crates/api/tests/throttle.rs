use std::sync::Arc;

use api::auth::hash_password;
use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{Request, Response, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
use bf_core::totp;
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct horse";
const NEW_PASSWORD: &str = "a brand new passphrase";

fn app(pool: PgPool) -> Router {
    let ring = KeyRing::from_pairs("t1", &[("t1", [7u8; 32])]).unwrap();
    let state = AppState {
        pool,
        jwt_keys: api::auth::JwtKeys::single("test-secret").into(),
        version_cache: Default::default(),
        vault: Arc::new(Vault::new(ring)),
        secure_cookies: true,
        limiter: std::sync::Arc::new(api::limit::Limiter::new(
            std::time::Duration::from_millis(1),
            100,
        )),
        tracker_url: Default::default(),
    };
    build(routes::routes(), state)
}

async fn add_user(pool: &PgPool, email: &str) -> User {
    let ws = Uuid::now_v7();
    create_workspace(pool, ws, "w", &ws.to_string())
        .await
        .unwrap();
    let user = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: email.into(),
        password_hash: hash_password(PASSWORD).unwrap(),
        role: UserRole::Admin,
    };
    create_user(pool, &user).await.unwrap();
    user
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Value,
) -> Response<Body> {
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
    app.clone().oneshot(req).await.unwrap()
}

async fn body_bytes(res: Response<Body>) -> (StatusCode, Vec<u8>) {
    let status = res.status();
    let b = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, b.to_vec())
}

async fn json_body(res: Response<Body>) -> Value {
    serde_json::from_slice(&body_bytes(res).await.1).unwrap_or(Value::Null)
}

/// Login; returns (access cookie, refresh cookie) pairs as `name=value`.
async fn login(app: &Router, email: &str, password: &str) -> Option<(String, String)> {
    let res = login_from(app, email, password, "9.9.9.9").await;
    if res.status() != StatusCode::OK {
        return None;
    }
    let find = |name: &str| {
        res.headers()
            .get_all(SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .find(|c| c.starts_with(name))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    };
    Some((find("bf_at="), find("bf_rt=")))
}

fn now_code(secret: &[u8]) -> String {
    format!(
        "{:06}",
        totp::code_at(secret, chrono::Utc::now().timestamp())
    )
}

/// Enrol + confirm TOTP; returns (secret, recovery codes).
async fn enrol(app: &Router, access: &str) -> (Vec<u8>, Vec<String>) {
    let res = call(
        app,
        "POST",
        "/api/v1/auth/totp/enrol",
        Some(access),
        json!({}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let b32 = json_body(res).await["secret_base32"]
        .as_str()
        .unwrap()
        .to_string();
    let secret = data_encoding::BASE32_NOPAD.decode(b32.as_bytes()).unwrap();
    let res = call(
        app,
        "POST",
        "/api/v1/auth/totp/confirm",
        Some(access),
        json!({"current_password": PASSWORD, "code": now_code(&secret)}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let codes = json_body(res).await["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    (secret, codes)
}

async fn reset(app: &Router, email: &str, code: &str, pw: &str) -> (StatusCode, Vec<u8>) {
    body_bytes(
        call(
            app,
            "POST",
            "/api/v1/auth/password/reset",
            None,
            json!({"email": email, "code": code, "new_password": pw}),
        )
        .await,
    )
    .await
}

/// A user with TOTP enrolled and a live session.
async fn setup(pool: PgPool) -> (Router, User, Vec<u8>, Vec<String>, (String, String)) {
    let user = add_user(&pool, "u@bf.test").await;
    let app = app(pool);
    let session = login(&app, &user.email, PASSWORD).await.unwrap();
    let (secret, codes) = enrol(&app, &session.0).await;
    (app, user, secret, codes, session)
}

async fn login_from(app: &Router, email: &str, password: &str, ip: &str) -> Response<Body> {
    let csrf = "bf_csrf=csrf-test-token";
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(HOST, "bf.test")
        .header(ORIGIN, "https://bf.test")
        .header(COOKIE, csrf)
        .header("x-csrf-token", "csrf-test-token")
        .header("x-forwarded-for", ip)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"email": email, "password": password}).to_string(),
        ))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn rewind_step(pool: &PgPool) {
    sqlx::query("UPDATE user_totp SET last_used_step = last_used_step - 5")
        .execute(pool)
        .await
        .unwrap();
}

async fn expire_locks(pool: &PgPool) {
    sqlx::query("UPDATE auth_throttle SET locked_until = now() - interval '1 second'")
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_locks_after_5_failures_and_looks_like_a_bad_code(pool: PgPool) {
    let (app, user, secret, _, _) = setup(pool.clone()).await;
    rewind_step(&pool).await;
    let mut bad = Vec::new();
    for _ in 0..5 {
        bad.push(reset(&app, &user.email, "000000", NEW_PASSWORD).await);
    }
    // A correct code is refused while locked, with the same status and body as a bad one.
    let locked = reset(&app, &user.email, &now_code(&secret), NEW_PASSWORD).await;
    assert_eq!(locked, bad[0]);
    assert_eq!(locked.0, StatusCode::UNAUTHORIZED);
    assert!(
        login(&app, &user.email, PASSWORD).await.is_some(),
        "password unchanged"
    );
    // Lock expires (locked_until in the past): the correct code works.
    expire_locks(&pool).await;
    let ok = reset(&app, &user.email, &now_code(&secret), NEW_PASSWORD).await;
    assert_eq!(ok.0, StatusCode::NO_CONTENT);
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_throttle WHERE scope = 'reset_user'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0, "success clears the throttle");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_second_lock_doubles(pool: PgPool) {
    let (app, user, _, _, _) = setup(pool.clone()).await;
    for _ in 0..5 {
        reset(&app, &user.email, "000000", NEW_PASSWORD).await;
    }
    let mins = |pool: PgPool| async move {
        sqlx::query_scalar::<_, f64>(
            "SELECT extract(epoch FROM locked_until - now())::float8 / 60 FROM auth_throttle",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .round() as i64
    };
    assert_eq!(mins(pool.clone()).await, 15);
    expire_locks(&pool).await;
    sqlx::query(
        "UPDATE auth_throttle SET failures = 0, window_started_at = now() - interval '1 hour'",
    )
    .execute(&pool)
    .await
    .unwrap();
    for _ in 0..5 {
        reset(&app, &user.email, "000000", NEW_PASSWORD).await;
    }
    assert_eq!(mins(pool).await, 30);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_throttle_is_per_user(pool: PgPool) {
    let (app, user, _, _, _) = setup(pool.clone()).await;
    let other = add_user(&pool, "other@bf.test").await;
    for _ in 0..5 {
        reset(&app, &user.email, "000000", NEW_PASSWORD).await;
    }
    let keys: Vec<String> = sqlx::query_scalar("SELECT key FROM auth_throttle")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(keys, vec![user.id.to_string()]);
    assert!(!keys.contains(&other.id.to_string()));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn login_backs_off_one_ip_not_the_account(pool: PgPool) {
    let user = add_user(&pool, "u@bf.test").await;
    let app = app(pool.clone());
    for _ in 0..5 {
        let r = login_from(&app, &user.email, "wrong", "1.1.1.1").await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    // The guessing address is slowed: even the right password gets the generic 401 ...
    let bad = body_bytes(login_from(&app, &user.email, "wrong", "1.1.1.1").await).await;
    let locked = body_bytes(login_from(&app, &user.email, PASSWORD, "1.1.1.1").await).await;
    assert_eq!(locked.0, StatusCode::UNAUTHORIZED);
    assert_eq!(locked, bad);
    // ... email case does not dodge it ...
    let upper = login_from(&app, "U@BF.TEST", PASSWORD, "1.1.1.1").await;
    assert_eq!(upper.status(), StatusCode::UNAUTHORIZED);
    // ... but the real user from another address is not locked out.
    let ok = login_from(&app, &user.email, PASSWORD, "2.2.2.2").await;
    assert_eq!(ok.status(), StatusCode::OK);
    // The backoff ends by itself.
    expire_locks(&pool).await;
    let ok = login_from(&app, &user.email, PASSWORD, "1.1.1.1").await;
    assert_eq!(ok.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn login_success_clears_backoff_and_unknown_email_is_tracked_alike(pool: PgPool) {
    let user = add_user(&pool, "u@bf.test").await;
    let app = app(pool.clone());
    for _ in 0..3 {
        login_from(&app, &user.email, "wrong", "1.1.1.1").await;
    }
    assert_eq!(
        login_from(&app, &user.email, PASSWORD, "1.1.1.1")
            .await
            .status(),
        StatusCode::OK
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_throttle")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    for _ in 0..3 {
        login_from(&app, "nobody@bf.test", "wrong", "1.1.1.1").await;
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_throttle WHERE scope = 'login_ip'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        n, 1,
        "unknown emails leave a row too, so rows do not reveal accounts"
    );
}
