//! Alertmanager sink and the rules-vs-metrics guard.
use std::sync::Arc;

use api::{AppState, build, routes};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bf_core::secrets::{KeyRing, Vault};
use sqlx::PgPool;
use tokio::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

/// The token lives in the process environment: tests that set it run one at a time.
static ENV: Mutex<()> = Mutex::const_new(());
const TOKEN: &str = "alert-token-123";

fn app(pool: PgPool) -> Router {
    let ring = KeyRing::from_pairs("t1", &[("t1", [7u8; 32])]).unwrap();
    build(
        routes::routes(),
        AppState {
            pool,
            jwt_keys: api::auth::JwtKeys::single("test-secret").into(),
            version_cache: Default::default(),
            vault: Arc::new(Vault::new(ring)),
            secure_cookies: true,
            limiter: Default::default(),
            tracker_url: Default::default(),
        },
    )
}

fn set_token(v: Option<&str>) {
    // SAFETY: serialised by ENV; no other thread reads the variable meanwhile.
    unsafe {
        match v {
            Some(v) => std::env::set_var("BF_ALERT_TOKEN", v),
            None => std::env::remove_var("BF_ALERT_TOKEN"),
        }
    }
}

async fn user(pool: &PgPool, role: &str, email: &str) -> Uuid {
    let ws = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', $2)")
        .bind(ws)
        .bind(ws.to_string())
        .execute(pool)
        .await
        .unwrap();
    let id = Uuid::now_v7();
    sqlx::query(&format!(
        "INSERT INTO users (id, workspace_id, email, password_hash, role) \
         VALUES ($1, $2, $3, 'h', '{role}')"
    ))
    .bind(id)
    .bind(ws)
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn req(auth: Option<&str>, body: String) -> Request<Body> {
    let mut b = Request::post("/api/v1/alerts/webhook").header("content-type", "application/json");
    if let Some(a) = auth {
        b = b.header("authorization", a);
    }
    b.body(Body::from(body)).unwrap()
}

fn payload(status: &str) -> String {
    format!(
        r#"{{"status":"{status}","alerts":[{{"status":"{status}","labels":{{"alertname":"TrackerDown","severity":"page"}},"annotations":{{"summary":"tracker is down"}}}}]}}"#
    )
}

async fn notes(pool: &PgPool, user: Uuid) -> Vec<(String, String, bool)> {
    sqlx::query_as::<_, (String, String, bool)>(
        "SELECT kind, body, read_at IS NOT NULL FROM notifications WHERE user_id = $1 \
         ORDER BY created_at, id",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[test]
fn token_compare() {
    use api::alerts::token_ok;
    assert!(token_ok(Some("abc"), Some("Bearer abc")));
    assert!(!token_ok(Some("abc"), Some("Bearer abd")));
    assert!(!token_ok(Some("abc"), Some("abc")));
    assert!(!token_ok(Some("abc"), None));
    assert!(!token_ok(None, Some("Bearer abc")));
    assert!(!token_ok(Some(""), Some("Bearer ")));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn alert_webhook_requires_token(pool: PgPool) {
    let _g = ENV.lock().await;
    set_token(Some(TOKEN));
    let app = app(pool);
    for auth in [None, Some("Bearer wrong"), Some(TOKEN)] {
        let res = app
            .clone()
            .oneshot(req(auth, payload("firing")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{auth:?}");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn alert_webhook_disabled_without_token(pool: PgPool) {
    let _g = ENV.lock().await;
    set_token(None);
    let res = app(pool)
        .oneshot(req(Some("Bearer anything"), payload("firing")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn alert_webhook_notifies_super_admins_only(pool: PgPool) {
    let _g = ENV.lock().await;
    set_token(Some(TOKEN));
    let (s1, s2) = (
        user(&pool, "super_admin", "s1@x.io").await,
        user(&pool, "super_admin", "s2@x.io").await,
    );
    let admin = user(&pool, "admin", "a@x.io").await;
    let aff = user(&pool, "affiliate", "f@x.io").await;
    let res = app(pool.clone())
        .oneshot(req(Some(&format!("Bearer {TOKEN}")), payload("firing")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    for s in [s1, s2] {
        let n = notes(&pool, s).await;
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].0, "alert");
        assert!(n[0].1.contains("TrackerDown") && n[0].1.contains("tracker is down"));
    }
    assert!(notes(&pool, admin).await.is_empty());
    assert!(notes(&pool, aff).await.is_empty());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn alert_webhook_resolved_status_marks_resolved(pool: PgPool) {
    let _g = ENV.lock().await;
    set_token(Some(TOKEN));
    let s = user(&pool, "super_admin", "s@x.io").await;
    let app = app(pool.clone());
    let auth = format!("Bearer {TOKEN}");
    for status in ["firing", "resolved"] {
        let res = app
            .clone()
            .oneshot(req(Some(&auth), payload(status)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
    let n = notes(&pool, s).await;
    assert_eq!(n.len(), 2);
    assert_eq!(
        (n[0].0.as_str(), n[0].2),
        ("alert", true),
        "firing marked read"
    );
    assert_eq!(n[1].0, "alert_resolved");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn alert_webhook_rejects_oversized_body(pool: PgPool) {
    let _g = ENV.lock().await;
    set_token(Some(TOKEN));
    let big = format!(r#"{{"alerts":[],"pad":"{}"}}"#, "x".repeat(70 * 1024));
    let res = app(pool)
        .oneshot(req(Some(&format!("Bearer {TOKEN}")), big))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// Every `bf_*` metric a rule uses must be rendered by the tracker or the api.
#[test]
fn rules_reference_only_exported_metrics() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rules = std::fs::read_to_string(root.join("deploy/monitoring/rules/bunfiiliate.rules.yml"))
        .unwrap();
    let mut exported = String::new();
    for f in ["crates/tracker/src/metrics.rs", "crates/api/src/metrics.rs"] {
        exported += &std::fs::read_to_string(root.join(f)).unwrap();
    }
    let mut used = std::collections::BTreeSet::new();
    let mut cur = String::new();
    for c in rules.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit() {
            cur.push(c);
        } else {
            if cur.starts_with("bf_") {
                used.insert(cur.clone());
            }
            cur.clear();
        }
    }
    assert!(!used.is_empty(), "no bf_ metrics found in rules");
    assert!(
        !used.contains("bf_clicks_default_rows"),
        "defined in C20, not here"
    );
    for m in &used {
        let base = m
            .trim_end_matches("_bucket")
            .trim_end_matches("_sum")
            .trim_end_matches("_count");
        assert!(
            exported.contains(&format!("\"{m}\"")) || exported.contains(&format!("\"{base}\"")),
            "rule uses unknown metric {m}"
        );
    }
}
