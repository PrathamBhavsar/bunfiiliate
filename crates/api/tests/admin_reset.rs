use std::sync::Arc;

use api::auth::hash_password;
use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{Request, Response, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
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
    };
    build(routes::routes(), state)
}

async fn add(pool: &PgPool, ws: Option<Uuid>, email: &str, role: UserRole) -> User {
    let user = User {
        id: Uuid::now_v7(),
        workspace_id: ws,
        email: email.into(),
        password_hash: hash_password(PASSWORD).unwrap(),
        role,
    };
    create_user(pool, &user).await.unwrap();
    user
}

async fn ws(pool: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    create_workspace(pool, id, "w", &id.to_string())
        .await
        .unwrap();
    id
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

async fn issue(app: &Router, access: &str, target: Uuid) -> (StatusCode, Value) {
    let res = call(
        app,
        "POST",
        &format!("/api/v1/users/{target}/password-reset-token"),
        Some(access),
        json!({}),
    )
    .await;
    let st = res.status();
    (st, json_body(res).await)
}

async fn redeem(app: &Router, token: &str, pw: &str) -> StatusCode {
    call(
        app,
        "POST",
        "/api/v1/auth/password/reset-with-token",
        None,
        json!({"token": token, "new_password": pw}),
    )
    .await
    .status()
}

struct Fx {
    app: Router,
    admin: User,
    aff: User,
    access: String,
}

async fn fx(pool: PgPool) -> Fx {
    let w = ws(&pool).await;
    let admin = add(&pool, Some(w), "a@bf.test", UserRole::Admin).await;
    let aff = add(&pool, Some(w), "aff@bf.test", UserRole::Affiliate).await;
    let app = app(pool);
    let (access, _) = login(&app, &admin.email, PASSWORD).await.unwrap();
    Fx {
        app,
        admin,
        aff,
        access,
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn admin_issues_token_and_user_resets(pool: PgPool) {
    let f = fx(pool.clone()).await;
    let (_, old_access) = (0, login(&f.app, &f.aff.email, PASSWORD).await.unwrap().0);
    let (st, b) = issue(&f.app, &f.access, f.aff.id).await;
    assert_eq!(st, StatusCode::CREATED);
    let token = b["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 43);
    assert!(b["expires_at"].as_str().is_some());
    // Only a hash is stored.
    let plain: i64 =
        sqlx::query_scalar("SELECT count(*) FROM password_reset_tokens WHERE token_hash = $1")
            .bind(token.as_bytes())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(plain, 0);
    assert_eq!(
        redeem(&f.app, &token, "short").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        redeem(&f.app, &token, NEW_PASSWORD).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        redeem(&f.app, &token, "another passphrase!").await,
        StatusCode::UNAUTHORIZED
    );
    assert!(login(&f.app, &f.aff.email, NEW_PASSWORD).await.is_some());
    assert!(login(&f.app, &f.aff.email, PASSWORD).await.is_none());
    let me = call(
        &f.app,
        "GET",
        "/api/v1/auth/me",
        Some(&old_access),
        json!({}),
    )
    .await;
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
    for action in ["auth.reset_token_issued", "auth.reset_token_used"] {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "{action}");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_notifies_in_app(pool: PgPool) {
    let f = fx(pool.clone()).await;
    let (_, b) = issue(&f.app, &f.access, f.aff.id).await;
    assert_eq!(
        redeem(&f.app, b["token"].as_str().unwrap(), NEW_PASSWORD).await,
        StatusCode::NO_CONTENT
    );
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM notifications WHERE user_id = $1 ORDER BY kind")
            .bind(f.aff.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(kinds, ["password_changed", "password_reset_token_issued"]);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn works_for_user_with_totp_enrolled(pool: PgPool) {
    let f = fx(pool.clone()).await;
    sqlx::query(
        "INSERT INTO user_totp (user_id, secret_enc, confirmed_at) VALUES ($1, 'x', now())",
    )
    .bind(f.aff.id)
    .execute(&pool)
    .await
    .unwrap();
    let (st, b) = issue(&f.app, &f.access, f.aff.id).await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(
        redeem(&f.app, b["token"].as_str().unwrap(), NEW_PASSWORD).await,
        StatusCode::NO_CONTENT
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'password_changed'",
    )
    .bind(f.aff.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn token_expires(pool: PgPool) {
    let f = fx(pool.clone()).await;
    let (_, b) = issue(&f.app, &f.access, f.aff.id).await;
    sqlx::query("UPDATE password_reset_tokens SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        redeem(&f.app, b["token"].as_str().unwrap(), NEW_PASSWORD).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        redeem(&f.app, "nonsense", NEW_PASSWORD).await,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn new_token_invalidates_previous(pool: PgPool) {
    let f = fx(pool).await;
    let (_, a) = issue(&f.app, &f.access, f.aff.id).await;
    let (_, b) = issue(&f.app, &f.access, f.aff.id).await;
    assert_eq!(
        redeem(&f.app, a["token"].as_str().unwrap(), NEW_PASSWORD).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        redeem(&f.app, b["token"].as_str().unwrap(), NEW_PASSWORD).await,
        StatusCode::NO_CONTENT
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn authorisation_rules(pool: PgPool) {
    let f = fx(pool.clone()).await;
    // self
    assert_eq!(
        issue(&f.app, &f.access, f.admin.id).await.0,
        StatusCode::FORBIDDEN
    );
    // other workspace
    let w2 = ws(&pool).await;
    let other = add(&pool, Some(w2), "o@bf.test", UserRole::Affiliate).await;
    assert_eq!(
        issue(&f.app, &f.access, other.id).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        issue(&f.app, &f.access, Uuid::now_v7()).await.0,
        StatusCode::NOT_FOUND
    );
    // equal or higher role
    let peer = add(&pool, f.admin.workspace_id, "peer@bf.test", UserRole::Admin).await;
    assert_eq!(
        issue(&f.app, &f.access, peer.id).await.0,
        StatusCode::FORBIDDEN
    );
    let root = add(
        &pool,
        f.admin.workspace_id,
        "root@bf.test",
        UserRole::SuperAdmin,
    )
    .await;
    assert_eq!(
        issue(&f.app, &f.access, root.id).await.0,
        StatusCode::FORBIDDEN
    );
    // affiliate cannot issue
    let (aff_access, _) = login(&f.app, &f.aff.email, PASSWORD).await.unwrap();
    assert_eq!(
        issue(&f.app, &aff_access, peer.id).await.0,
        StatusCode::FORBIDDEN
    );
    // super admin: anyone but self
    let (root_access, _) = login(&f.app, &root.email, PASSWORD).await.unwrap();
    assert_eq!(
        issue(&f.app, &root_access, other.id).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        issue(&f.app, &root_access, peer.id).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        issue(&f.app, &root_access, root.id).await.0,
        StatusCode::FORBIDDEN
    );
}
