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
    origin: bool,
    body: Value,
) -> Response<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(HOST, "bf.test");
    if origin {
        b = b.header(ORIGIN, "https://bf.test");
    }
    // Auth routes and cookie-authenticated writes need the double-submit CSRF token.
    let csrf = "bf_csrf=csrf-test-token";
    b = b.header(
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

async fn json_body(res: Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn login(app: &Router, email: &str) -> String {
    let res = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        true,
        json!({"email": email, "password": PASSWORD}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let c = res
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|c| c.starts_with("bf_at="))
        .unwrap();
    c.split(';').next().unwrap().to_string()
}

async fn post(app: &Router, path: &str, cookie: &str, body: Value) -> Response<Body> {
    call(
        app,
        "POST",
        &format!("/api/v1/auth/totp/{path}"),
        Some(cookie),
        true,
        body,
    )
    .await
}

async fn get_status(app: &Router, cookie: &str) -> Value {
    let res = call(
        app,
        "GET",
        "/api/v1/auth/totp/status",
        Some(cookie),
        false,
        json!({}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    json_body(res).await
}

fn secret_of(enrol: &Value) -> Vec<u8> {
    data_encoding::BASE32_NOPAD
        .decode(enrol["secret_base32"].as_str().unwrap().as_bytes())
        .unwrap()
}

fn now_code(secret: &[u8]) -> String {
    format!(
        "{:06}",
        totp::code_at(secret, chrono::Utc::now().timestamp())
    )
}

fn code_body(code: &str) -> Value {
    json!({"current_password": PASSWORD, "code": code})
}

/// Enrol and confirm; returns (secret, recovery codes).
async fn enrolled(app: &Router, cookie: &str) -> (Vec<u8>, Vec<String>) {
    let (s, c, _) = enrolled_with_code(app, cookie).await;
    (s, c)
}

/// Like [`enrolled`], also returning the code confirm consumed.
async fn enrolled_with_code(app: &Router, cookie: &str) -> (Vec<u8>, Vec<String>, String) {
    let res = post(app, "enrol", cookie, json!({})).await;
    assert_eq!(res.status(), StatusCode::OK);
    let secret = secret_of(&json_body(res).await);
    let used = now_code(&secret);
    let res = post(app, "confirm", cookie, code_body(&used)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let codes = json_body(res).await["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    (secret, codes, used)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_returns_uri_and_pending(pool: PgPool) {
    add_user(&pool, "a@x.io").await;
    let app = app(pool);
    let at = login(&app, "a@x.io").await;
    let res = post(&app, "enrol", &at, json!({})).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert!(
        body["otpauth_uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/")
    );
    assert_eq!(secret_of(&body).len(), 20);
    assert_eq!(get_status(&app, &at).await["enrolled"], false);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_confirm_returns_10_argon2_codes(pool: PgPool) {
    let user = add_user(&pool, "a@x.io").await;
    let app = app(pool.clone());
    let at = login(&app, "a@x.io").await;
    let (_, codes) = enrolled(&app, &at).await;
    assert_eq!(codes.len(), 10);
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT code_hash FROM user_recovery_codes WHERE user_id = $1")
            .bind(user.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 10);
    for h in &rows {
        assert!(h.starts_with("$argon2"), "salted argon2, got {h}");
        assert!(!codes.contains(h));
    }
    // Salted: the same code never hashes the same way twice.
    assert_eq!(
        rows.iter().collect::<std::collections::HashSet<_>>().len(),
        10
    );
    let enc: String = sqlx::query_scalar("SELECT secret_enc FROM user_totp WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(enc.starts_with("bfenc1:"));
    let st = get_status(&app, &at).await;
    assert_eq!(st["enrolled"], true);
    assert_eq!(st["recovery_codes_remaining"], 10);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_confirm_wrong_code_or_password_401(pool: PgPool) {
    add_user(&pool, "a@x.io").await;
    let app = app(pool);
    let at = login(&app, "a@x.io").await;
    let res = post(&app, "enrol", &at, json!({})).await;
    let secret = secret_of(&json_body(res).await);
    let good = now_code(&secret);
    let bad = format!(
        "{:06}",
        (good.parse::<u32>().unwrap() + 500_000) % 1_000_000
    );
    let res = post(&app, "confirm", &at, code_body(&bad)).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = post(
        &app,
        "confirm",
        &at,
        json!({"current_password": "nope", "code": good}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(get_status(&app, &at).await["enrolled"], false);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_confirm_replay_same_step_rejected(pool: PgPool) {
    add_user(&pool, "a@x.io").await;
    let app = app(pool);
    let at = login(&app, "a@x.io").await;
    let (_, _, used) = enrolled_with_code(&app, &at).await;
    // Same code, same step, already consumed by confirm.
    let res = post(&app, "disable", &at, code_body(&used)).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(get_status(&app, &at).await["enrolled"], true);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_twice_after_confirm_409(pool: PgPool) {
    add_user(&pool, "a@x.io").await;
    let app = app(pool);
    let at = login(&app, "a@x.io").await;
    enrolled(&app, &at).await;
    let res = post(&app, "enrol", &at, json!({})).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_pending_secret_is_replaced(pool: PgPool) {
    add_user(&pool, "a@x.io").await;
    let app = app(pool);
    let at = login(&app, "a@x.io").await;
    let first = secret_of(&json_body(post(&app, "enrol", &at, json!({})).await).await);
    let second = secret_of(&json_body(post(&app, "enrol", &at, json!({})).await).await);
    assert_ne!(first, second);
    // Only the latest secret confirms.
    let res = post(&app, "confirm", &at, code_body(&now_code(&second))).await;
    assert_eq!(res.status(), StatusCode::OK);
}

/// A code from a later step than the one confirm used (disable would replay otherwise).
fn next_step_code(secret: &[u8]) -> String {
    format!(
        "{:06}",
        totp::code_at(secret, chrono::Utc::now().timestamp() + 30)
    )
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_disable_requires_password_and_code(pool: PgPool) {
    let user = add_user(&pool, "a@x.io").await;
    let app = app(pool.clone());
    let at = login(&app, "a@x.io").await;
    let (secret, _) = enrolled(&app, &at).await;
    let code = next_step_code(&secret);
    let res = post(
        &app,
        "disable",
        &at,
        json!({"current_password": "nope", "code": code}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = post(&app, "disable", &at, code_body("000000")).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = post(&app, "disable", &at, code_body(&code)).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    for t in ["user_totp", "user_recovery_codes"] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {t} WHERE user_id = $1"))
            .bind(user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "{t}");
    }
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'totp.disable' AND actor_id = $1",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_regenerate_invalidates_old_codes(pool: PgPool) {
    let user = add_user(&pool, "a@x.io").await;
    let app = app(pool.clone());
    let at = login(&app, "a@x.io").await;
    let (secret, old) = enrolled(&app, &at).await;
    let old_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM user_recovery_codes WHERE user_id = $1")
            .bind(user.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    let res = post(
        &app,
        "recovery-codes/regenerate",
        &at,
        code_body(&next_step_code(&secret)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let new: Vec<String> = json_body(res).await["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(new.len(), 10);
    assert!(new.iter().all(|c| !old.contains(c)));
    let remaining: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM user_recovery_codes WHERE user_id = $1")
            .bind(user.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(remaining.len(), 10);
    assert!(remaining.iter().all(|id| !old_ids.contains(id)));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_routes_require_auth_and_origin(pool: PgPool) {
    add_user(&pool, "a@x.io").await;
    let app = app(pool);
    let at = login(&app, "a@x.io").await;
    for path in ["enrol", "confirm", "disable", "recovery-codes/regenerate"] {
        let uri = format!("/api/v1/auth/totp/{path}");
        let res = call(&app, "POST", &uri, None, true, code_body("123456")).await;
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "{path} unauthenticated"
        );
        let res = call(&app, "POST", &uri, Some(&at), false, code_body("123456")).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{path} no origin");
    }
    let res = call(
        &app,
        "GET",
        "/api/v1/auth/totp/status",
        None,
        false,
        json!({}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_secret_never_returned_after_enrol(pool: PgPool) {
    let user = add_user(&pool, "a@x.io").await;
    let app = app(pool.clone());
    let at = login(&app, "a@x.io").await;
    let res = post(&app, "enrol", &at, json!({})).await;
    let b32 = json_body(res).await["secret_base32"]
        .as_str()
        .unwrap()
        .to_string();
    let confirm_codes = {
        let secret = data_encoding::BASE32_NOPAD.decode(b32.as_bytes()).unwrap();
        post(&app, "confirm", &at, code_body(&now_code(&secret))).await
    };
    let confirm_body = json_body(confirm_codes).await.to_string();
    assert!(!confirm_body.contains(&b32));
    let status = get_status(&app, &at).await.to_string();
    assert!(!status.contains(&b32));
    let me = call(&app, "GET", "/api/v1/auth/me", Some(&at), false, json!({})).await;
    assert!(!json_body(me).await.to_string().contains(&b32));
    let stored: String = sqlx::query_scalar("SELECT secret_enc FROM user_totp WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!stored.contains(&b32));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_secret_bound_to_user(pool: PgPool) {
    let a = add_user(&pool, "a@x.io").await;
    let b = add_user(&pool, "b@x.io").await;
    let app = app(pool.clone());
    let (at, bt) = (login(&app, "a@x.io").await, login(&app, "b@x.io").await);
    let (secret, _) = enrolled(&app, &at).await;
    let sealed: String = sqlx::query_scalar("SELECT secret_enc FROM user_totp WHERE user_id = $1")
        .bind(a.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    post(&app, "enrol", &bt, json!({})).await;
    sqlx::query("UPDATE user_totp SET secret_enc = $2 WHERE user_id = $1")
        .bind(b.id)
        .bind(sealed)
        .execute(&pool)
        .await
        .unwrap();
    // Ciphertext copied to another user's row cannot be opened: no code of A's works for B.
    let res = post(&app, "confirm", &bt, code_body(&now_code(&secret))).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_replayed_step_cannot_be_claimed_twice(pool: PgPool) {
    let user = add_user(&pool, "a@x.io").await;
    add_user(&pool, "b@x.io").await;
    let app = app(pool.clone());
    let at = login(&app, "a@x.io").await;
    enrolled(&app, &at).await;
    let last: i64 = sqlx::query_scalar("SELECT last_used_step FROM user_totp WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!bf_db::totp::claim_step(&pool, user.id, last).await.unwrap());
    assert!(
        !bf_db::totp::claim_step(&pool, user.id, last - 1)
            .await
            .unwrap()
    );
    assert!(
        bf_db::totp::claim_step(&pool, user.id, last + 1)
            .await
            .unwrap()
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn totp_enrol_reencrypt_seals_and_rewraps_totp_secret(pool: PgPool) {
    let user = add_user(&pool, "a@x.io").await;
    // Legacy plaintext secret, as written with BF_ALLOW_PLAINTEXT_SECRETS=1.
    sqlx::query("INSERT INTO user_totp (user_id, secret_enc) VALUES ($1, 'JBSWY3DPEHPK3PXP')")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    let ring = KeyRing::from_pairs("t1", &[("t1", [7u8; 32])]).unwrap();
    let report = api::secrets::reencrypt(&pool, &ring, false).await.unwrap();
    assert_eq!(report.encrypted, 1);
    let (enc, at): (String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT secret_enc, secret_enc_at FROM user_totp WHERE user_id = $1")
            .bind(user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(enc.starts_with("bfenc1:t1:") && at.is_some());
    let v = Vault::new(ring);
    assert_eq!(
        v.open_col("user_totp", "secret_enc", user.id, &enc)
            .unwrap(),
        "JBSWY3DPEHPK3PXP"
    );
}
