//! Payout rail credentials: sealed at rest, redacted on read, 422 when a rail has none.

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
            jwt_keys: api::auth::JwtKeys::single(SECRET).into(),
            version_cache: Default::default(),
            tracker_url: Default::default(),
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
            .header(COOKIE, format!("{cookie}; bf_csrf=t"));
        if method != Method::GET {
            b = b
                .header(ORIGIN, "https://bf.test")
                .header("x-csrf-token", "t");
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

async fn admin(pool: &PgPool) -> (String, Uuid, Uuid) {
    let ws = Uuid::now_v7();
    create_workspace(pool, ws, "w", &ws.to_string())
        .await
        .unwrap();
    let u = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: format!("{}@x.io", Uuid::now_v7()),
        password_hash: "unused".into(),
        role: UserRole::Admin,
    };
    create_user(pool, &u).await.unwrap();
    let cookie = format!(
        "bf_at={}",
        issue_access(SECRET, &Principal::from(&u)).unwrap()
    );
    (cookie, ws, u.id)
}

const CREDS: &str = "/api/v1/payouts/rails/credentials";
const PAYPAL_DEL: &str = "/api/v1/payouts/rails/credentials?rail=paypal_payouts";

fn paypal_body() -> Value {
    json!({"rail": "paypal_payouts", "credentials": {"client_id": "AaClientIdXYZ", "client_secret": "EsSecretXYZ"}, "sandbox": true})
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_credentials_are_sealed_and_never_returned(pool: PgPool) {
    let api = Api::new(&pool);
    let (c, ws, _) = admin(&pool).await;
    let (st, body) = api.call(Method::PUT, CREDS, &c, Some(paypal_body())).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(!body.to_string().contains("EsSecretXYZ"));

    let (id, raw, marker): (Uuid, String, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT id, creds, creds_enc_at FROM payout_rail_credentials WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(raw.starts_with("bfenc1:") && !raw.contains("EsSecretXYZ") && marker.is_some());
    let opened = vault()
        .open_col("payout_rail_credentials", "creds", id, &raw)
        .unwrap();
    assert!(opened.contains("EsSecretXYZ"));

    let (_, rails) = api
        .call(Method::GET, "/api/v1/payouts/rails", &c, None)
        .await;
    let text = rails.to_string();
    assert!(
        !text.contains("EsSecretXYZ")
            && !text.contains("AaClientIdXYZ")
            && !text.contains("bfenc1"),
        "{text}"
    );
    let paypal = rails
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["rail"] == "paypal_payouts")
        .unwrap();
    assert_eq!(
        (&paypal["configured"], &paypal["available"]),
        (&json!(true), &json!(true))
    );
    let wise = rails
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["rail"] == "wise")
        .unwrap();
    assert_eq!(wise["available"], json!(false));

    // Replacing keeps one row, re-sealed under the same row id.
    let (st, _) = api.call(Method::PUT, CREDS, &c, Some(paypal_body())).await;
    assert_eq!(st, StatusCode::OK);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM payout_rail_credentials")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);

    // The audit trail never carries the secret.
    let diffs: Vec<String> = sqlx::query_scalar("SELECT COALESCE(diff::text, '') FROM audit_log")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(diffs.iter().all(|d| !d.contains("EsSecretXYZ")));

    let (st, _) = api.call(Method::DELETE, PAYPAL_DEL, &c, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_credentials_validate_keys_and_rail(pool: PgPool) {
    let api = Api::new(&pool);
    let (c, _, _) = admin(&pool).await;
    for bad in [
        json!({"rail": "paypal_payouts", "credentials": {"client_id": "x"}}),
        json!({"rail": "paypal_payouts", "credentials": {"client_id": "x", "client_secret": "y", "extra": "z"}}),
        json!({"rail": "paypal_payouts", "credentials": {"client_id": "x", "client_secret": " "}}),
    ] {
        let (st, _) = api.call(Method::PUT, CREDS, &c, Some(bad)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    for rail in ["mock", "manual_csv", "venmo"] {
        let mut body = paypal_body();
        body["rail"] = json!(rail);
        let (st, _) = api.call(Method::PUT, CREDS, &c, Some(body)).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_credentials_are_per_workspace(pool: PgPool) {
    let api = Api::new(&pool);
    let (c1, _, _) = admin(&pool).await;
    let (c2, _, _) = admin(&pool).await;
    api.call(Method::PUT, CREDS, &c1, Some(paypal_body())).await;
    let (_, rails) = api
        .call(Method::GET, "/api/v1/payouts/rails", &c2, None)
        .await;
    let paypal = rails
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["rail"] == "paypal_payouts")
        .unwrap();
    assert_eq!(paypal["configured"], json!(false));
    let (st, _) = api.call(Method::DELETE, PAYPAL_DEL, &c2, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_mark_paid_unconfigured_rail_422_and_bad_rail_400(pool: PgPool) {
    let api = Api::new(&pool);
    let (c, ws, user) = admin(&pool).await;
    let program = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
         VALUES ($1, $2, 'p', 'USD', $3, 's')",
    )
    .bind(program)
    .bind(ws)
    .bind(program.to_string())
    .execute(&pool)
    .await
    .unwrap();
    let batch = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO payout_batches (id, program_id, created_by, status) VALUES ($1, $2, $3, 'approved')",
    )
    .bind(batch)
    .bind(program)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();
    let uri = format!("/api/v1/payouts/batches/{batch}/mark-paid");
    let (st, _) = api
        .call(
            Method::POST,
            &uri,
            &c,
            Some(json!({"rail": "paypal_payouts"})),
        )
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = api
        .call(Method::POST, &uri, &c, Some(json!({"rail": "venmo"})))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let status: String =
        sqlx::query_scalar("SELECT status::text FROM payout_batches WHERE id = $1")
            .bind(batch)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "approved");
    // No body keeps the manual behaviour (an empty approved batch is simply paid).
    let (st, body) = api.call(Method::POST, &uri, &c, None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "paid");
    assert_eq!(body["outcome"]["batch_paid"], true);
}

#[test]
fn payout_rail_provider_status_mapping() {
    use api::payout_rails::{paypal_batch_status, paypal_item_status, wise_status};
    use bf_core::RailStatus::*;
    assert_eq!(paypal_batch_status("SUCCESS"), Paid);
    assert_eq!(paypal_batch_status("PENDING"), Pending);
    assert_eq!(paypal_batch_status("PROCESSING"), Pending);
    assert_eq!(paypal_batch_status("DENIED"), Failed);
    assert_eq!(paypal_item_status("SUCCESS"), Paid);
    assert_eq!(paypal_item_status("UNCLAIMED"), Pending);
    assert_eq!(paypal_item_status("RETURNED"), Failed);
    assert_eq!(wise_status("outgoing_payment_sent"), Paid);
    assert_eq!(wise_status("processing"), Pending);
    assert_eq!(wise_status("funds_refunded"), Failed);
}
