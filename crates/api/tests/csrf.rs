use api::auth::hash_password;
use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{Request, Response, StatusCode};
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct horse";
const TOKEN: &str = "tok-123";

fn app(pool: PgPool) -> Router {
    let state = AppState {
        pool,
        jwt_keys: api::auth::JwtKeys::single("test-secret").into(),
        version_cache: Default::default(),
        vault: std::sync::Arc::new(bf_core::secrets::Vault::plaintext()),
        secure_cookies: true,
        limiter: Default::default(),
        tracker_url: Default::default(),
    };
    build(routes::routes(), state)
}

#[derive(Default)]
struct R {
    method: &'static str,
    uri: &'static str,
    origin: Option<&'static str>,
    cookie: Option<String>,
    header: Option<&'static str>,
    bearer: bool,
    body: Option<Value>,
}

async fn send(app: &Router, r: R) -> Response<Body> {
    let mut b = Request::builder()
        .method(r.method)
        .uri(r.uri)
        .header(HOST, "bf.test");
    if let Some(o) = r.origin {
        b = b.header(ORIGIN, o);
    }
    if let Some(c) = r.cookie {
        b = b.header(COOKIE, c);
    }
    if let Some(t) = r.header {
        b = b.header("x-csrf-token", t);
    }
    if r.bearer {
        b = b.header(AUTHORIZATION, "Bearer x");
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
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn logout(cookie: Option<&str>, header: Option<&'static str>) -> R {
    R {
        method: "POST",
        uri: "/api/v1/auth/logout",
        origin: Some("https://bf.test"),
        cookie: cookie.map(String::from),
        header,
        ..Default::default()
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn mutation_without_csrf_header_403(pool: PgPool) {
    let app = app(pool);
    let res = send(&app, logout(Some(&format!("bf_csrf={TOKEN}")), None)).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(res).await["error"]["code"], "csrf");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn mismatched_csrf_403(pool: PgPool) {
    let app = app(pool);
    let res = send(
        &app,
        logout(Some(&format!("bf_csrf={TOKEN}")), Some("other")),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = send(&app, logout(Some("bf_at=x"), Some(TOKEN))).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "header without cookie");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn valid_csrf_and_origin_ok(pool: PgPool) {
    let app = app(pool);
    let res = send(&app, logout(Some(&format!("bf_csrf={TOKEN}")), Some(TOKEN))).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn valid_csrf_but_bad_origin_403(pool: PgPool) {
    let app = app(pool);
    let mut r = logout(Some(&format!("bf_csrf={TOKEN}")), Some(TOKEN));
    r.origin = Some("https://evil.test");
    let res = send(&app, r).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(res).await["error"]["code"], "forbidden");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn get_needs_no_csrf(pool: PgPool) {
    let app = app(pool);
    let r = R {
        method: "GET",
        uri: "/api/v1/auth/me",
        ..Default::default()
    };
    // 401 (no session), not 403 csrf.
    assert_eq!(send(&app, r).await.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn bearer_requests_are_exempt(pool: PgPool) {
    let app = app(pool);
    let mut r = logout(None, None);
    r.uri = "/api/v1/auth/logout";
    r.bearer = true;
    assert_eq!(send(&app, r).await.status(), StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn login_requires_csrf(pool: PgPool) {
    let ws = Uuid::now_v7();
    create_workspace(&pool, ws, "w", &ws.to_string())
        .await
        .unwrap();
    let user = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: "a@b.test".into(),
        password_hash: hash_password(PASSWORD).unwrap(),
        role: UserRole::Admin,
    };
    create_user(&pool, &user).await.unwrap();
    let app = app(pool);
    let login = |cookie: Option<&str>, header| R {
        method: "POST",
        uri: "/api/v1/auth/login",
        origin: Some("https://bf.test"),
        cookie: cookie.map(String::from),
        header,
        body: Some(json!({ "email": "a@b.test", "password": PASSWORD })),
        ..Default::default()
    };
    let res = send(&app, login(None, None)).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(res).await["error"]["code"], "csrf");
    let res = send(&app, login(Some(&format!("bf_csrf={TOKEN}")), Some(TOKEN))).await;
    assert_eq!(res.status(), StatusCode::OK);
    // login rotates a csrf cookie alongside the session
    let sets: Vec<_> = res
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(sets.iter().any(|c| c.starts_with("bf_csrf=")));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn csrf_endpoint_sets_cookie_flags(pool: PgPool) {
    let app = app(pool);
    let r = R {
        method: "GET",
        uri: "/api/v1/auth/csrf",
        ..Default::default()
    };
    let res = send(&app, r).await;
    assert_eq!(res.status(), StatusCode::OK);
    let c = res
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(c.starts_with("bf_csrf="));
    assert!(c.contains("Secure") && c.contains("SameSite=Lax") && c.contains("Path=/"));
    assert!(!c.contains("HttpOnly"));
    let token = json_body(res).await["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 43, "32 bytes b64url");
    assert!(c.contains(&token));
    // existing cookie is reused, not re-set
    let r = R {
        method: "GET",
        uri: "/api/v1/auth/csrf",
        cookie: Some(format!("bf_csrf={token}")),
        ..Default::default()
    };
    let res = send(&app, r).await;
    assert!(res.headers().get(SET_COOKIE).is_none());
    assert_eq!(json_body(res).await["token"], token);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn headers_present_on_ok_and_error_responses(pool: PgPool) {
    let app = app(pool);
    for (uri, status) in [
        ("/api/openapi.json", StatusCode::OK),
        ("/api/v1/auth/me", StatusCode::UNAUTHORIZED),
        ("/api/v1/nope", StatusCode::NOT_FOUND),
    ] {
        let res = send(
            &app,
            R {
                method: "GET",
                uri,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(res.status(), status, "{uri}");
        let h = res.headers();
        assert_eq!(h["x-content-type-options"], "nosniff", "{uri}");
        assert_eq!(h["x-frame-options"], "DENY", "{uri}");
        assert_eq!(h["referrer-policy"], "strict-origin-when-cross-origin");
        assert!(h.contains_key("permissions-policy"));
        assert_eq!(h["cross-origin-opener-policy"], "same-origin");
    }
    // csrf 403 from middleware also carries them
    let res = send(&app, logout(Some("bf_at=x"), None)).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(res.headers()["x-frame-options"], "DENY");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_responses_no_store(pool: PgPool) {
    let app = app(pool);
    for uri in ["/api/v1/auth/me", "/api/v1/auth/csrf"] {
        let res = send(
            &app,
            R {
                method: "GET",
                uri,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(res.headers()["cache-control"], "no-store", "{uri}");
    }
    let res = send(
        &app,
        R {
            method: "GET",
            uri: "/api/openapi.json",
            ..Default::default()
        },
    )
    .await;
    assert!(!res.headers().contains_key("cache-control"));
}
