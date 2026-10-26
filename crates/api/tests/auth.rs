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
