//! Secrets at rest: ciphertext in the database, plaintext only once on create.

use std::sync::Arc;

use api::auth::{Principal, issue_access};
use api::{AppState, app};
use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use axum::http::{Method, Request, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
use bf_db::auth::{User, UserRole, create_user, create_workspace};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-secret";

fn vault() -> Arc<Vault> {
    let ring = KeyRing::from_pairs("t1", &[("t1", [5u8; 32])]).unwrap();
    Arc::new(Vault::new(ring))
}

struct Api(Router);

impl Api {
    fn new(pool: &PgPool) -> Self {
        Self(app(AppState {
            pool: pool.clone(),
            jwt_secret: SECRET.into(),
            vault: vault(),
            secure_cookies: true,
            limiter: Default::default(),
        }))
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        cookie: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method.clone())
            .uri(uri)
            .header(HOST, "bf.test")
            .header(COOKIE, cookie);
        if method != Method::GET {
            b = b.header(ORIGIN, "https://bf.test");
        }
        let body = match body {
            Some(v) => {
                b = b.header(CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self.0.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

async fn admin(pool: &PgPool) -> String {
    let ws = Uuid::now_v7();
    create_workspace(pool, ws, "w", "w").await.unwrap();
    let u = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: format!("{}@x.io", Uuid::now_v7()),
        password_hash: "unused".into(),
        role: UserRole::Admin,
    };
    create_user(pool, &u).await.unwrap();
    format!(
        "bf_at={}",
        issue_access(SECRET, &Principal::from(&u)).unwrap()
    )
}

async fn new_program(api: &Api, cookie: &str, name: &str) -> Value {
    let (st, p) = api
        .call(
            Method::POST,
            "/api/v1/programs",
            cookie,
            Some(json!({ "name": name, "currency": "USD" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{p}");
    p
}

async fn stored(pool: &PgPool, id: &str, col: &str) -> String {
    let id: Uuid = id.parse().unwrap();
    sqlx::query_scalar(&format!("SELECT {col} FROM programs WHERE id = $1"))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn new_program_secrets_are_ciphertext(pool: PgPool) {
    let api = Api::new(&pool);
    let c = admin(&pool).await;
    let p = new_program(&api, &c, "P").await;
    let shown = p["webhook_secret"].as_str().unwrap();
    assert!(shown.starts_with("whsec_"), "plaintext is shown once");
    let raw = stored(&pool, p["id"].as_str().unwrap(), "webhook_secret").await;
    assert!(raw.starts_with("bfenc1:"), "{raw}");
    assert!(!raw.contains(shown));
    let id: Uuid = p["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        vault()
            .open_col("programs", "webhook_secret", id, &raw)
            .unwrap(),
        shown
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn rotate_and_provider_secrets_are_sealed(pool: PgPool) {
    let api = Api::new(&pool);
    let c = admin(&pool).await;
    let p = new_program(&api, &c, "P").await;
    let id = p["id"].as_str().unwrap();
    let (st, r) = api
        .call(
            Method::POST,
            &format!("/api/v1/programs/{id}/secrets/rotate"),
            &c,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let plain = r["webhook_secret"].as_str().unwrap();
    assert_ne!(plain, p["webhook_secret"].as_str().unwrap());
    assert!(
        stored(&pool, id, "webhook_secret")
            .await
            .starts_with("bfenc1:")
    );

    let (st, u) = api
        .call(
            Method::PATCH,
            &format!("/api/v1/programs/{id}"),
            &c,
            Some(json!({ "stripe_secret": "sk_live_abc", "paddle_secret": "pdl_xyz" })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{u}");
    assert!(u.get("stripe_secret").is_none() && u.get("webhook_secret").is_none());
    for col in ["stripe_secret", "paddle_secret"] {
        let raw = stored(&pool, id, col).await;
        assert!(raw.starts_with("bfenc1:"), "{col}");
        assert!(!raw.contains("sk_live_abc") && !raw.contains("pdl_xyz"));
    }
    // empty string clears
    let (st, _) = api
        .call(
            Method::PATCH,
            &format!("/api/v1/programs/{id}"),
            &c,
            Some(json!({ "stripe_secret": "" })),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let pid: Uuid = id.parse().unwrap();
    let cleared: Option<String> =
        sqlx::query_scalar("SELECT stripe_secret FROM programs WHERE id = $1")
            .bind(pid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cleared, None);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_endpoint_secret_is_sealed_and_shown_once(pool: PgPool) {
    let api = Api::new(&pool);
    let c = admin(&pool).await;
    let (st, e) = api
        .call(
            Method::POST,
            "/api/v1/webhooks/endpoints",
            &c,
            Some(json!({ "url": "https://example.com/h" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{e}");
    let plain = e["secret"].as_str().unwrap();
    assert!(plain.starts_with("whsec_"));
    let id: Uuid = e["id"].as_str().unwrap().parse().unwrap();
    let raw: String = sqlx::query_scalar("SELECT secret FROM webhook_endpoints WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(raw.starts_with("bfenc1:") && !raw.contains(plain));
    assert_eq!(
        vault()
            .open_col("webhook_endpoints", "secret", id, &raw)
            .unwrap(),
        plain
    );
    for uri in [
        "/api/v1/webhooks/endpoints".to_string(),
        format!("/api/v1/webhooks/endpoints/{id}"),
    ] {
        let (_, body) = api.call(Method::GET, &uri, &c, None).await;
        let text = body.to_string();
        assert!(
            !text.contains("whsec_") && !text.contains("bfenc1"),
            "{text}"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_and_get_never_leak_secrets(pool: PgPool) {
    let api = Api::new(&pool);
    let c = admin(&pool).await;
    let p = new_program(&api, &c, "P").await;
    let id = p["id"].as_str().unwrap();
    let _ = api
        .call(
            Method::PATCH,
            &format!("/api/v1/programs/{id}"),
            &c,
            Some(json!({ "stripe_secret": "sk_live_abc" })),
        )
        .await;
    for uri in [
        "/api/v1/programs".to_string(),
        format!("/api/v1/programs/{id}"),
    ] {
        let (_, body) = api.call(Method::GET, &uri, &c, None).await;
        let text = body.to_string();
        for needle in ["whsec_", "bfenc1", "sk_live_abc"] {
            assert!(!text.contains(needle), "{uri} leaks {needle}");
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ciphertext_is_bound_to_its_row(pool: PgPool) {
    let api = Api::new(&pool);
    let c = admin(&pool).await;
    let a = new_program(&api, &c, "A").await;
    let b = new_program(&api, &c, "B").await;
    let raw_a = stored(&pool, a["id"].as_str().unwrap(), "webhook_secret").await;
    let b_id: Uuid = b["id"].as_str().unwrap().parse().unwrap();
    // Copy A's ciphertext into B: it must not open there.
    assert!(
        vault()
            .open_col("programs", "webhook_secret", b_id, &raw_a)
            .is_err()
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn legacy_plaintext_still_opens(pool: PgPool) {
    let api = Api::new(&pool);
    let c = admin(&pool).await;
    let p = new_program(&api, &c, "P").await;
    let id: Uuid = p["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE programs SET webhook_secret = 'whsec_legacy' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let raw = stored(&pool, &id.to_string(), "webhook_secret").await;
    assert_eq!(
        vault()
            .open_col("programs", "webhook_secret", id, &raw)
            .unwrap(),
        "whsec_legacy"
    );
}
