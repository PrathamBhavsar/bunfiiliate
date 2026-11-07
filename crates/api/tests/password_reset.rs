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
        limiter: Default::default(),
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
    let res = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        json!({"email": email, "password": password}),
    )
    .await;
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

async fn me_status(app: &Router, access: &str) -> StatusCode {
    call(app, "GET", "/api/v1/auth/me", Some(access), json!({}))
        .await
        .status()
}

/// A user with TOTP enrolled and a live session.
async fn setup(pool: PgPool) -> (Router, User, Vec<u8>, Vec<String>, (String, String)) {
    let user = add_user(&pool, "u@bf.test").await;
    let app = app(pool);
    let session = login(&app, &user.email, PASSWORD).await.unwrap();
    let (secret, codes) = enrol(&app, &session.0).await;
    (app, user, secret, codes, session)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_with_totp_succeeds_and_revokes_sessions(pool: PgPool) {
    let (app, user, secret, _, (access, refresh)) = setup(pool.clone()).await;
    assert_eq!(me_status(&app, &access).await, StatusCode::OK);
    // The confirm step used the current code; a reset needs a newer one, so rewind the step.
    sqlx::query("UPDATE user_totp SET last_used_step = last_used_step - 5")
        .execute(&pool)
        .await
        .unwrap();
    let (status, _) = reset(&app, &user.email, &now_code(&secret), NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let res = call(
        &app,
        "POST",
        "/api/v1/auth/refresh",
        Some(&refresh),
        json!({}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(me_status(&app, &access).await, StatusCode::UNAUTHORIZED);
    assert!(login(&app, &user.email, NEW_PASSWORD).await.is_some());
    assert!(login(&app, &user.email, PASSWORD).await.is_none());

    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM notifications WHERE user_id = $1")
            .bind(user.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(kinds, ["password_changed"]);
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'auth.password_reset'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audits, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_with_recovery_code_succeeds_once(pool: PgPool) {
    let (app, user, _, codes, _) = setup(pool.clone()).await;
    let (status, _) = reset(&app, &user.email, &codes[0], NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_recovery_codes WHERE user_id = $1 AND used_at IS NOT NULL",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(used, 1);
    let (status, _) = reset(&app, &user.email, &codes[0], "yet another passphrase").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(login(&app, &user.email, NEW_PASSWORD).await.is_some());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unknown_email_and_no_totp_same_response_as_bad_code(pool: PgPool) {
    let (app, user, _, _, _) = setup(pool.clone()).await;
    let plain = add_user(&pool, "plain@bf.test").await;
    let bad = reset(&app, &user.email, "000000", NEW_PASSWORD).await;
    let unknown = reset(&app, "nobody@bf.test", "000000", NEW_PASSWORD).await;
    let no_totp = reset(&app, &plain.email, "000000", NEW_PASSWORD).await;
    assert_eq!(bad.0, StatusCode::UNAUTHORIZED);
    assert_eq!(bad, unknown);
    assert_eq!(bad, no_totp);
    let bad_rec = reset(&app, &user.email, "aaaaa-bbbbb", NEW_PASSWORD).await;
    let unk_rec = reset(&app, "nobody@bf.test", "aaaaa-bbbbb", NEW_PASSWORD).await;
    assert_eq!(bad_rec, unk_rec);
    assert_eq!(bad_rec, bad);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_code_replay_rejected(pool: PgPool) {
    let (app, user, secret, _, _) = setup(pool.clone()).await;
    // `setup` consumed a step at confirm. Replay exactly that step's code (not "now", which may
    // have rolled into the next step on a slow machine): it must not reset.
    let step: i64 = sqlx::query_scalar("SELECT last_used_step FROM user_totp WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let replay = format!("{:06}", bf_core::totp::code_at(&secret, step * 30));
    let (status, _) = reset(&app, &user.email, &replay, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(login(&app, &user.email, PASSWORD).await.is_some());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn weak_password_400_and_code_not_consumed(pool: PgPool) {
    let (app, user, _, codes, _) = setup(pool.clone()).await;
    for pw in ["short", &"x".repeat(257)] {
        let (status, _) = reset(&app, &user.email, &codes[0], pw).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    assert!(login(&app, &user.email, PASSWORD).await.is_some());
    let (status, _) = reset(&app, &user.email, &codes[0], NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_is_atomic(pool: PgPool) {
    let (app, user, _, codes, _) = setup(pool.clone()).await;
    // Fail the last write of the transaction (the notification) and expect a full rollback.
    sqlx::query(
        "CREATE FUNCTION boom() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'boom'; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER boom BEFORE INSERT ON notifications FOR EACH ROW EXECUTE FUNCTION boom()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, _) = reset(&app, &user.email, &codes[0], NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_recovery_codes WHERE user_id = $1 AND used_at IS NOT NULL",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(used, 0);
    assert!(login(&app, &user.email, PASSWORD).await.is_some());
    let ver: i32 = sqlx::query_scalar("SELECT token_version FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ver, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ip_rate_limit_429(pool: PgPool) {
    let app = app(pool);
    let mut last = StatusCode::OK;
    for i in 0..11 {
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/password/reset")
            .header(HOST, "bf.test")
            .header(ORIGIN, "https://bf.test")
            .header(COOKIE, "bf_csrf=csrf-test-token")
            .header("x-csrf-token", "csrf-test-token")
            .header("x-forwarded-for", "203.0.113.9")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"email": format!("n{i}@bf.test"), "code": "000000", "new_password": NEW_PASSWORD})
                    .to_string(),
            ))
            .unwrap();
        last = app.clone().oneshot(req).await.unwrap().status();
        if i < 10 {
            assert_eq!(last, StatusCode::UNAUTHORIZED);
        }
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_requires_csrf(pool: PgPool) {
    let app = app(pool);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/password/reset")
        .header(HOST, "bf.test")
        .header(ORIGIN, "https://bf.test")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"email": "a@b.c", "code": "000000", "new_password": NEW_PASSWORD}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}
