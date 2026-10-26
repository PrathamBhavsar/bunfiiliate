use api::auth::{Admin, Auth, hash_password};
use api::bootstrap::bootstrap;
use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{Request, Response, StatusCode};
use axum::routing::get;
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const HOST_NAME: &str = "bf.test";
const ORIGIN_OK: &str = "https://bf.test";
const PASSWORD: &str = "correct horse";

async fn admin_only(_: Auth<Admin>) -> &'static str {
    "ok"
}

fn app(pool: PgPool) -> Router {
    let state = AppState {
        pool,
        jwt_secret: "test-secret".into(),
        secure_cookies: true,
        limiter: Default::default(),
    };
    build(
        routes::routes().route("/admin-only", get(admin_only)),
        state,
    )
}

async fn add_user(pool: &PgPool, email: &str, role: UserRole) -> User {
    let ws = Uuid::now_v7();
    create_workspace(pool, ws, "w", &ws.to_string())
        .await
        .unwrap();
    let user = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: email.into(),
        password_hash: hash_password(PASSWORD).unwrap(),
        role,
    };
    create_user(pool, &user).await.unwrap();
    user
}

struct Req {
    method: &'static str,
    uri: &'static str,
    body: Option<Value>,
    cookies: Vec<String>,
    origin: Option<&'static str>,
}

fn post(uri: &'static str) -> Req {
    Req {
        method: "POST",
        uri,
        body: None,
        cookies: vec![],
        origin: Some(ORIGIN_OK),
    }
}

fn get_req(uri: &'static str) -> Req {
    Req {
        method: "GET",
        origin: None,
        ..post(uri)
    }
}

impl Req {
    fn json(mut self, v: Value) -> Self {
        self.body = Some(v);
        self
    }
    fn cookie(mut self, c: &str) -> Self {
        self.cookies.push(c.to_string());
        self
    }
    fn origin(mut self, o: Option<&'static str>) -> Self {
        self.origin = o;
        self
    }
}

async fn send(app: &Router, r: Req) -> Response<Body> {
    let mut b = Request::builder()
        .method(r.method)
        .uri(r.uri)
        .header(HOST, HOST_NAME);
    if let Some(o) = r.origin {
        b = b.header(ORIGIN, o);
    }
    if !r.cookies.is_empty() {
        b = b.header(COOKIE, r.cookies.join("; "));
    }
    let body = match r.body {
        Some(v) => {
            b = b.header(CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    app.clone().oneshot(b.body(body).unwrap()).await.unwrap()
}

async fn json_body(res: Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn set_cookies(res: &Response<Body>) -> Vec<String> {
    res.headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

/// `name=value` of the named Set-Cookie, ready to send back.
fn cookie_pair(res: &Response<Body>, name: &str) -> String {
    let c = set_cookies(res)
        .into_iter()
        .find(|c| c.starts_with(&format!("{name}=")))
        .unwrap();
    c.split(';').next().unwrap().to_string()
}

async fn login(app: &Router, email: &str) -> Response<Body> {
    let req = post("/api/v1/auth/login").json(json!({ "email": email, "password": PASSWORD }));
    send(app, req).await
}

fn refresh_with(rt: &str) -> Req {
    post("/api/v1/auth/refresh").cookie(rt)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_login_ok(pool: PgPool) {
    let user = add_user(&pool, "a@x.io", UserRole::Admin).await;
    let app = app(pool.clone());
    let res = login(&app, "a@x.io").await;
    assert_eq!(res.status(), StatusCode::OK);
    let (at, rt) = (cookie_pair(&res, "bf_at"), cookie_pair(&res, "bf_rt"));
    let body = json_body(res).await;
    assert_eq!(body["email"], "a@x.io");
    assert_eq!(body["role"], "admin");
    assert!(rt.starts_with("bf_rt="));

    let me = send(&app, get_req("/api/v1/auth/me").cookie(&at)).await;
    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(json_body(me).await["id"], user.id.to_string());

    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'auth.login' AND actor_id = $1",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_login_bad_password(pool: PgPool) {
    add_user(&pool, "a@x.io", UserRole::Admin).await;
    let app = app(pool);
    for (email, password) in [("a@x.io", "wrong"), ("nobody@x.io", PASSWORD)] {
        let req = post("/api/v1/auth/login").json(json!({ "email": email, "password": password }));
        let res = send(&app, req).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert!(set_cookies(&res).is_empty());
        let body = json_body(res).await;
        assert_eq!(body["error"]["code"], "unauthorized");
        assert_eq!(body["error"]["message"], "invalid credentials");
    }
    let bad_json = send(
        &app,
        post("/api/v1/auth/login").json(json!({ "email": "a@x.io" })),
    )
    .await;
    assert_eq!(bad_json.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(bad_json).await["error"]["code"], "bad_request");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_cookie_flags(pool: PgPool) {
    add_user(&pool, "a@x.io", UserRole::Admin).await;
    let res = login(&app(pool), "a@x.io").await;
    let cookies = set_cookies(&res);
    let at = cookies.iter().find(|c| c.starts_with("bf_at=")).unwrap();
    let rt = cookies.iter().find(|c| c.starts_with("bf_rt=")).unwrap();
    for c in [at, rt] {
        assert!(c.contains("HttpOnly"), "{c}");
        assert!(c.contains("SameSite=Strict"), "{c}");
        assert!(c.contains("Secure"), "{c}");
    }
    assert!(at.contains("Path=/;") && at.contains("Max-Age=900"), "{at}");
    assert!(rt.contains("Path=/api/v1/auth;"), "{rt}");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_refresh_rotates(pool: PgPool) {
    add_user(&pool, "a@x.io", UserRole::Admin).await;
    let app = app(pool.clone());
    let rt1 = cookie_pair(&login(&app, "a@x.io").await, "bf_rt");

    let res = send(&app, refresh_with(&rt1)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let (at2, rt2) = (cookie_pair(&res, "bf_at"), cookie_pair(&res, "bf_rt"));
    assert_ne!(rt1, rt2);
    let me = send(&app, get_req("/api/v1/auth/me").cookie(&at2)).await;
    assert_eq!(me.status(), StatusCode::OK);

    let (total, revoked, linked): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*), count(revoked_at), count(replaced_by) FROM refresh_tokens",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((total, revoked, linked), (2, 1, 1));

    // the new token rotates again
    assert_eq!(
        send(&app, refresh_with(&rt2)).await.status(),
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_refresh_reuse_revokes_family(pool: PgPool) {
    add_user(&pool, "a@x.io", UserRole::Admin).await;
    let app = app(pool.clone());
    let rt1 = cookie_pair(&login(&app, "a@x.io").await, "bf_rt");
    let rt2 = cookie_pair(&send(&app, refresh_with(&rt1)).await, "bf_rt");
    age_rotations(&pool).await;

    let reuse = send(&app, refresh_with(&rt1)).await;
    assert_eq!(reuse.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        json_body(reuse).await["error"]["message"],
        "refresh token reused"
    );

    // the legitimate descendant is dead too
    assert_eq!(
        send(&app, refresh_with(&rt2)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let live: i64 =
        sqlx::query_scalar("SELECT count(*) FROM refresh_tokens WHERE revoked_at IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(live, 0);

    // a fresh login starts a new family and still works
    let rt3 = cookie_pair(&login(&app, "a@x.io").await, "bf_rt");
    assert_eq!(
        send(&app, refresh_with(&rt3)).await.status(),
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_logout_revokes_and_clears(pool: PgPool) {
    add_user(&pool, "a@x.io", UserRole::Admin).await;
    let app = app(pool);
    let rt = cookie_pair(&login(&app, "a@x.io").await, "bf_rt");

    let res = send(&app, post("/api/v1/auth/logout").cookie(&rt)).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(set_cookies(&res).iter().all(|c| c.contains("Max-Age=0")));
    assert_eq!(
        send(&app, refresh_with(&rt)).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_role_guard_403(pool: PgPool) {
    add_user(&pool, "aff@x.io", UserRole::Affiliate).await;
    add_user(&pool, "adm@x.io", UserRole::Admin).await;
    let app = app(pool);

    let none = send(&app, get_req("/admin-only")).await;
    assert_eq!(none.status(), StatusCode::UNAUTHORIZED);

    let aff = cookie_pair(&login(&app, "aff@x.io").await, "bf_at");
    let denied = send(&app, get_req("/admin-only").cookie(&aff)).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(denied).await["error"]["code"], "forbidden");

    let adm = cookie_pair(&login(&app, "adm@x.io").await, "bf_at");
    assert_eq!(
        send(&app, get_req("/admin-only").cookie(&adm))
            .await
            .status(),
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_origin_mismatch_403(pool: PgPool) {
    add_user(&pool, "a@x.io", UserRole::Admin).await;
    let app = app(pool);
    let creds = json!({ "email": "a@x.io", "password": PASSWORD });

    for origin in [Some("https://evil.test"), Some("null"), None] {
        let req = post("/api/v1/auth/login")
            .json(creds.clone())
            .origin(origin);
        let res = send(&app, req).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "origin {origin:?}");
        assert_eq!(json_body(res).await["error"]["message"], "origin mismatch");
    }
    // scheme is ignored (TLS terminates at Caddy); reads need no Origin
    let req = post("/api/v1/auth/login")
        .json(creds)
        .origin(Some("http://bf.test"));
    assert_eq!(send(&app, req).await.status(), StatusCode::OK);
    assert_eq!(
        send(&app, get_req("/api/v1/auth/me")).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_bootstrap_creates_one_super_admin(pool: PgPool) {
    assert!(
        bootstrap(&pool, "root", PASSWORD, "Acme Inc")
            .await
            .is_err()
    );
    assert!(
        bootstrap(&pool, "root@x.io", "short", "Acme Inc")
            .await
            .is_err()
    );
    bootstrap(&pool, "root@x.io", PASSWORD, "Acme Inc")
        .await
        .unwrap();
    assert!(
        bootstrap(&pool, "two@x.io", PASSWORD, "Other")
            .await
            .is_err()
    );

    let slug: String = sqlx::query_scalar("SELECT slug FROM workspaces")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(slug, "acme-inc");
    let res = login(&app(pool), "root@x.io").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json_body(res).await["role"], "super_admin");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_openapi_served(pool: PgPool) {
    let res = send(&app(pool), get_req("/api/openapi.json")).await;
    assert_eq!(res.status(), StatusCode::OK);
    let doc = json_body(res).await;
    for p in ["login", "refresh", "logout", "me"] {
        assert!(doc["paths"][format!("/api/v1/auth/{p}")].is_object(), "{p}");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_bootstrap_concurrent_creates_one(pool: PgPool) {
    let (a, b) = tokio::join!(
        bootstrap(&pool, "a@x.io", PASSWORD, "A"),
        bootstrap(&pool, "b@x.io", PASSWORD, "B"),
    );
    assert_eq!(a.is_ok() as u8 + b.is_ok() as u8, 1);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'super_admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

/// Pretend the rotations happened a minute ago, past the reuse grace window.
async fn age_rotations(pool: &PgPool) {
    sqlx::query("UPDATE refresh_tokens SET revoked_at = now() - interval '1 minute' WHERE revoked_at IS NOT NULL")
        .execute(pool)
        .await
        .unwrap();
}

async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_refresh_reuse_inside_grace_does_not_revoke_family(pool: PgPool) {
    add_user(&pool, "a@x.io", UserRole::Admin).await;
    let app = app(pool.clone());
    let rt1 = cookie_pair(&login(&app, "a@x.io").await, "bf_rt");
    let rt2 = cookie_pair(&send(&app, refresh_with(&rt1)).await, "bf_rt");

    // A second tab (or a lost response) replays rt1 right after the rotation.
    let again = send(&app, refresh_with(&rt1)).await;
    assert_eq!(again.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        json_body(again).await["error"]["message"],
        "invalid refresh token"
    );

    // The family survives and its live token still rotates.
    assert_eq!(
        send(&app, refresh_with(&rt2)).await.status(),
        StatusCode::OK
    );
    assert_eq!(audit_count(&pool, "auth.refresh_reuse").await, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_refresh_reuse_after_grace_is_audited(pool: PgPool) {
    add_user(&pool, "a@x.io", UserRole::Admin).await;
    let app = app(pool.clone());
    let rt1 = cookie_pair(&login(&app, "a@x.io").await, "bf_rt");
    send(&app, refresh_with(&rt1)).await;
    age_rotations(&pool).await;
    assert_eq!(
        send(&app, refresh_with(&rt1)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(audit_count(&pool, "auth.refresh_reuse").await, 1);
}

/// A UUIDv7 minted `days` ago, the id a family would have had then.
fn family_id_aged(days: u64) -> Uuid {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - days * 86_400;
    Uuid::new_v7(uuid::Timestamp::from_unix(uuid::NoContext, secs, 0))
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_refresh_family_has_an_absolute_lifetime(pool: PgPool) {
    add_user(&pool, "a@x.io", UserRole::Admin).await;
    let app = app(pool.clone());

    // 89 days in: still refreshes, but the new token stops at day 90, not day 119.
    let rt = cookie_pair(&login(&app, "a@x.io").await, "bf_rt");
    sqlx::query("UPDATE refresh_tokens SET family_id = $1")
        .bind(family_id_aged(89))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(send(&app, refresh_with(&rt)).await.status(), StatusCode::OK);
    let days_left: i64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM max(expires_at) - now())::bigint / 86400 FROM refresh_tokens WHERE revoked_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        days_left <= 1,
        "capped at the family end, got {days_left} days"
    );

    // 91 days in: refused although the token itself has not expired.
    let rt = cookie_pair(&login(&app, "a@x.io").await, "bf_rt");
    sqlx::query("UPDATE refresh_tokens SET family_id = $1 WHERE revoked_at IS NULL AND expires_at > now() + interval '10 days'")
        .bind(family_id_aged(91))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        send(&app, refresh_with(&rt)).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_login_is_rate_limited_per_email(pool: PgPool) {
    add_user(&pool, "rl-login@x.io", UserRole::Admin).await;
    let app = app(pool);
    let mut last = StatusCode::OK;
    for _ in 0..12 {
        last = login(&app, "rl-login@x.io").await.status();
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    // Another address is unaffected.
    assert_eq!(
        send(
            &app,
            post("/api/v1/auth/login").json(json!({ "email": "nobody@x.io", "password": "x" }))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_unknown_email_is_the_same_401_as_wrong_password(pool: PgPool) {
    add_user(&pool, "known@x.io", UserRole::Admin).await;
    let app = app(pool);
    let wrong = send(
        &app,
        post("/api/v1/auth/login").json(json!({ "email": "known@x.io", "password": "nope" })),
    )
    .await;
    let unknown = send(
        &app,
        post("/api/v1/auth/login").json(json!({ "email": "ghost@x.io", "password": "nope" })),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(wrong).await, json_body(unknown).await);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_oversized_body_is_refused(pool: PgPool) {
    let app = app(pool);
    let big = "x".repeat(70 * 1024);
    let res = send(
        &app,
        post("/api/v1/auth/login").json(json!({ "email": "a@x.io", "password": big })),
    )
    .await;
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
