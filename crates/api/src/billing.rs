//! Billing: provider adapters (mock, LemonSqueezy), checkout, the signed provider webhook, the
//! subscription view and plan-limit metering.
//!
//! Provider events are deduplicated by event id (`billing_events`), applied in the order of the
//! provider's `updated_at`, and mapped to a workspace through `custom_data.workspace_id` that the
//! checkout stamped. Only `mock` and `lemonsqueezy` are accepted as a provider. The webhook
//! secret is sealed in `billing_providers`.
//!
//! Plan-limit checks (`enforce`) read then insert without a lock, so two concurrent creates can
//! overshoot a limit by one. Accepted: limits are a soft commercial guard, not a security
//! boundary.

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use bf_core::billing::{Limit, Provider, SubStatus, SubscriptionEvent, verify, would_exceed};
use bf_db::billing as db;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Auth, SuperAdmin};
use crate::error::{AppError, AppJson, ErrorBody};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/billing/checkout", post(checkout))
        .route("/api/v1/billing/subscription", get(subscription))
        .route("/api/v1/billing/providers", get(list_providers))
        .route("/api/v1/billing/providers/{provider}", put(set_provider))
        .route("/api/v1/webhooks/billing/{provider}", post(webhook))
}

// ---- provider adapters -------------------------------------------------------------------

#[derive(Debug)]
pub enum ProviderError {
    /// Not configured on this instance (no API key, no variant mapping).
    Unavailable(String),
    /// The provider call failed.
    Upstream(String),
    /// The webhook body is not a usable event.
    BadEvent(String),
}

pub struct CheckoutInput<'a> {
    pub workspace_id: Uuid,
    pub plan_key: &'a str,
    /// The provider's id for the plan, from `billing_plans`.
    pub variant: Option<&'a str>,
}

pub enum CheckoutOutcome {
    /// The plan is already active; no redirect.
    Activated,
    /// Send the admin to this hosted checkout.
    Redirect(String),
}

/// A billing rail. Adapters do the HTTP; signature checks and types live in `bf-core`.
#[allow(async_fn_in_trait)]
pub trait BillingProvider {
    fn kind(&self) -> Provider;
    /// Header carrying the hex HMAC of the raw body.
    fn signature_header(&self) -> &'static str {
        "x-signature"
    }
    async fn checkout(&self, c: &CheckoutInput<'_>) -> Result<CheckoutOutcome, ProviderError>;
    fn parse_event(&self, body: &[u8]) -> Result<SubscriptionEvent, ProviderError>;
}

/// Activates immediately; its events use the LemonSqueezy wire format.
pub struct MockProvider;

impl BillingProvider for MockProvider {
    fn kind(&self) -> Provider {
        Provider::Mock
    }
    async fn checkout(&self, _: &CheckoutInput<'_>) -> Result<CheckoutOutcome, ProviderError> {
        Ok(CheckoutOutcome::Activated)
    }
    fn parse_event(&self, body: &[u8]) -> Result<SubscriptionEvent, ProviderError> {
        parse_lemonsqueezy(body)
    }
}

pub struct LemonSqueezy {
    api_key: Option<String>,
    store_id: Option<String>,
    base: String,
}

impl LemonSqueezy {
    pub fn from_env() -> Self {
        let get = |k| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            api_key: get("LEMONSQUEEZY_API_KEY"),
            store_id: get("LEMONSQUEEZY_STORE_ID"),
            base: get("LEMONSQUEEZY_API_URL")
                .unwrap_or_else(|| "https://api.lemonsqueezy.com".into()),
        }
    }
}

impl BillingProvider for LemonSqueezy {
    fn kind(&self) -> Provider {
        Provider::LemonSqueezy
    }
    async fn checkout(&self, c: &CheckoutInput<'_>) -> Result<CheckoutOutcome, ProviderError> {
        let (Some(key), Some(store)) = (&self.api_key, &self.store_id) else {
            return Err(ProviderError::Unavailable(
                "LemonSqueezy is not configured (LEMONSQUEEZY_API_KEY, LEMONSQUEEZY_STORE_ID)"
                    .into(),
            ));
        };
        let variant = c
            .variant
            .ok_or_else(|| ProviderError::Unavailable("plan has no LemonSqueezy variant".into()))?;
        // JSON:API. `custom` comes back as `meta.custom_data` on every webhook for the checkout.
        let body = json!({"data": {"type": "checkouts",
            "attributes": {"checkout_data": {"custom": {
                "workspace_id": c.workspace_id.to_string(), "plan_key": c.plan_key}}},
            "relationships": {
                "store": {"data": {"type": "stores", "id": store}},
                "variant": {"data": {"type": "variants", "id": variant}}}}});
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let res = client
            .post(format!("{}/v1/checkouts", self.base))
            .bearer_auth(key)
            .header("accept", "application/vnd.api+json")
            .header("content-type", "application/vnd.api+json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = res.status();
        let bytes = res
            .bytes()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if !status.is_success() {
            return Err(ProviderError::Upstream(format!(
                "checkout failed: {status}"
            )));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::Upstream("bad checkout response".into()))?;
        let url = v["data"]["attributes"]["url"]
            .as_str()
            .filter(|u| u.starts_with("https://"))
            .ok_or_else(|| ProviderError::Upstream("no checkout url".into()))?;
        Ok(CheckoutOutcome::Redirect(url.to_string()))
    }
    fn parse_event(&self, body: &[u8]) -> Result<SubscriptionEvent, ProviderError> {
        parse_lemonsqueezy(body)
    }
}

fn parse_time(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// LemonSqueezy subscription webhook:
/// `{"meta":{"event_name","webhook_id"?,"custom_data":{"workspace_id","plan_key"}},
///   "data":{"id","attributes":{"status","updated_at","renews_at","ends_at"}}}`.
/// LemonSqueezy documents no event id, so a missing `meta.webhook_id` falls back to the body
/// hash: a verbatim retry dedupes, and `updated_at` ordering covers the rest.
pub fn parse_lemonsqueezy(body: &[u8]) -> Result<SubscriptionEvent, ProviderError> {
    let bad = |m: &str| ProviderError::BadEvent(m.to_string());
    let v: Value = serde_json::from_slice(body).map_err(|_| bad("not json"))?;
    let name = v["meta"]["event_name"].as_str().unwrap_or("");
    if !name.starts_with("subscription_") {
        return Err(bad("not a subscription event"));
    }
    let workspace_id = v["meta"]["custom_data"]["workspace_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| bad("custom_data.workspace_id missing"))?;
    let attrs = &v["data"]["attributes"];
    let status = attrs["status"]
        .as_str()
        .and_then(SubStatus::from_lemonsqueezy)
        .ok_or_else(|| bad("unknown status"))?;
    let updated_at = parse_time(&attrs["updated_at"]).ok_or_else(|| bad("updated_at missing"))?;
    let event_id = match v["meta"]["webhook_id"].as_str().filter(|s| !s.is_empty()) {
        Some(id) if id.len() <= 128 => id.to_string(),
        Some(_) => return Err(bad("webhook_id too long")),
        None => {
            use sha2::{Digest, Sha256};
            data_encoding::HEXLOWER.encode(&Sha256::digest(body))
        }
    };
    let id = v["data"]["id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| v["data"]["id"].as_i64().map(|n| n.to_string()));
    let period_end = if status == SubStatus::Canceled {
        parse_time(&attrs["ends_at"])
    } else {
        parse_time(&attrs["renews_at"])
    };
    Ok(SubscriptionEvent {
        event_id,
        workspace_id,
        provider_subscription_id: id,
        status,
        plan_key: v["meta"]["custom_data"]["plan_key"]
            .as_str()
            .map(str::to_owned),
        updated_at,
        current_period_end: period_end,
    })
}

// ---- metering ----------------------------------------------------------------------------

/// 402 naming the limit if creating one more `kind` would pass the workspace's plan.
pub async fn enforce(pool: &PgPool, ws: Uuid, kind: Limit) -> Result<(), AppError> {
    let plan = db::plan_for_workspace(pool, ws).await?;
    let max = match kind {
        Limit::Programs => plan.max_programs.map(i64::from),
        Limit::Affiliates => plan.max_affiliates.map(i64::from),
        Limit::MonthlyClicks => plan.max_monthly_clicks,
    };
    if max.is_none() {
        return Ok(());
    }
    let usage = db::usage_for_workspace(pool, ws).await?;
    let used = match kind {
        Limit::Programs => usage.programs,
        Limit::Affiliates => usage.affiliates,
        Limit::MonthlyClicks => usage.monthly_clicks,
    };
    if would_exceed(max, used) {
        return Err(AppError::PaymentRequired(format!(
            "plan limit reached: {}",
            kind.name()
        )));
    }
    Ok(())
}

// ---- routes ------------------------------------------------------------------------------

#[derive(Deserialize, ToSchema)]
pub struct CheckoutRequest {
    plan_key: String,
    /// `mock` or `lemonsqueezy`; default `mock` when configured, else `lemonsqueezy`.
    provider: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct CheckoutResponse {
    /// `true` when the plan is already active (mock).
    activated: bool,
    /// Hosted checkout to send the admin to.
    url: Option<String>,
}

fn parse_provider(s: &str) -> Result<Provider, AppError> {
    Provider::parse(s).ok_or_else(|| AppError::BadRequest("unknown billing provider".into()))
}

fn provider_err(e: ProviderError) -> AppError {
    match e {
        ProviderError::Unavailable(m) => AppError::Unprocessable(m),
        ProviderError::Upstream(m) => {
            tracing::error!(error = %m, "billing provider call failed");
            AppError::Unprocessable("billing provider call failed".into())
        }
        ProviderError::BadEvent(m) => AppError::BadRequest(m),
    }
}

/// Start a plan change. `mock` switches the plan at once; `lemonsqueezy` returns a hosted checkout.
#[utoipa::path(
    post,
    path = "/api/v1/billing/checkout",
    tag = "billing",
    security(("cookie" = [])),
    request_body = CheckoutRequest,
    responses(
        (status = 200, body = CheckoutResponse),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 422, body = ErrorBody),
    ),
)]
pub async fn checkout(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<CheckoutRequest>,
) -> Result<Json<CheckoutResponse>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let provider = match req.provider.as_deref() {
        Some(p) => parse_provider(p)?,
        None => Provider::Mock,
    };
    let plan = db::plan_by_key(&s.pool, &req.plan_key)
        .await?
        .ok_or(AppError::NotFound("plan not found"))?;
    let outcome = match provider {
        Provider::Mock => {
            // The mock grants any plan for free: only when a super admin configured it.
            if db::provider_id(&s.pool, "mock").await?.is_none() {
                return Err(AppError::Unprocessable(
                    "the mock billing provider is not enabled".into(),
                ));
            }
            run_checkout(&MockProvider, ws, &plan, &s.pool).await?
        }
        Provider::LemonSqueezy => {
            run_checkout(&LemonSqueezy::from_env(), ws, &plan, &s.pool).await?
        }
    };
    let mut tx = s.pool.begin().await?;
    if matches!(outcome, CheckoutOutcome::Activated) {
        db::activate_plan(&mut *tx, ws, plan.id, provider.as_str()).await?;
    }
    audit::record(
        &mut *tx,
        &auth.principal,
        "billing.checkout",
        "plan",
        plan.id,
        Some(json!({"plan_key": plan.key, "provider": provider.as_str()})),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(match outcome {
        CheckoutOutcome::Activated => CheckoutResponse {
            activated: true,
            url: None,
        },
        CheckoutOutcome::Redirect(url) => CheckoutResponse {
            activated: false,
            url: Some(url),
        },
    }))
}

async fn run_checkout<P: BillingProvider>(
    p: &P,
    ws: Uuid,
    plan: &db::Plan,
    pool: &PgPool,
) -> Result<CheckoutOutcome, AppError> {
    let variant = db::lemonsqueezy_variant(pool, plan.id).await?;
    p.checkout(&CheckoutInput {
        workspace_id: ws,
        plan_key: &plan.key,
        variant: variant.as_deref(),
    })
    .await
    .map_err(provider_err)
}

#[derive(Serialize, ToSchema)]
pub struct PlanView {
    key: String,
    name: String,
    price_cents: i64,
    currency: String,
    max_programs: Option<i32>,
    max_affiliates: Option<i32>,
    max_monthly_clicks: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct UsageView {
    programs: i64,
    affiliates: i64,
    monthly_clicks: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SubscriptionView {
    plan: PlanView,
    usage: UsageView,
    provider: String,
    status: String,
    current_period_end: Option<DateTime<Utc>>,
}

/// The workspace's plan, status and usage against its limits.
#[utoipa::path(
    get,
    path = "/api/v1/billing/subscription",
    tag = "billing",
    security(("cookie" = [])),
    responses((status = 200, body = SubscriptionView)),
)]
pub async fn subscription(
    State(s): State<AppState>,
    auth: Auth<Admin>,
) -> Result<Json<SubscriptionView>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let p = db::plan_for_workspace(&s.pool, ws).await?;
    let u = db::usage_for_workspace(&s.pool, ws).await?;
    let sub = db::subscription_for_workspace(&s.pool, ws).await?;
    Ok(Json(SubscriptionView {
        plan: PlanView {
            key: p.key,
            name: p.name,
            price_cents: p.price_cents,
            currency: p.currency,
            max_programs: p.max_programs,
            max_affiliates: p.max_affiliates,
            max_monthly_clicks: p.max_monthly_clicks,
        },
        usage: UsageView {
            programs: u.programs,
            affiliates: u.affiliates,
            monthly_clicks: u.monthly_clicks,
        },
        provider: sub.as_ref().map_or("mock".into(), |x| x.provider.clone()),
        status: sub.as_ref().map_or("active".into(), |x| x.status.clone()),
        current_period_end: sub.and_then(|x| x.current_period_end),
    }))
}

#[derive(Serialize, ToSchema)]
pub struct ProviderView {
    provider: String,
    /// The webhook secret is write-only; this only says one is stored.
    webhook_secret_set: bool,
}

/// Providers with a webhook secret configured. The secret itself is never returned.
#[utoipa::path(
    get,
    path = "/api/v1/billing/providers",
    tag = "billing",
    security(("cookie" = [])),
    responses((status = 200, body = Vec<ProviderView>)),
)]
pub async fn list_providers(
    State(s): State<AppState>,
    _auth: Auth<SuperAdmin>,
) -> Result<Json<Vec<ProviderView>>, AppError> {
    let set = db::configured_providers(&s.pool).await?;
    Ok(Json(
        ["lemonsqueezy", "mock"]
            .iter()
            .map(|p| ProviderView {
                provider: (*p).into(),
                webhook_secret_set: set.iter().any(|x| x == p),
            })
            .collect(),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct SetSecret {
    webhook_secret: String,
}

/// Set a provider's webhook secret (sealed at rest). Enables the provider.
#[utoipa::path(
    put,
    path = "/api/v1/billing/providers/{provider}",
    tag = "billing",
    security(("cookie" = [])),
    params(("provider" = String, Path, description = "mock or lemonsqueezy")),
    request_body = SetSecret,
    responses(
        (status = 200, body = ProviderView),
        (status = 400, body = ErrorBody),
        (status = 409, body = ErrorBody),
    ),
)]
pub async fn set_provider(
    State(s): State<AppState>,
    auth: Auth<SuperAdmin>,
    Path(provider): Path<String>,
    AppJson(req): AppJson<SetSecret>,
) -> Result<Json<ProviderView>, AppError> {
    let p = parse_provider(&provider)?;
    if req.webhook_secret.len() < 16 || req.webhook_secret.len() > 256 {
        return Err(AppError::BadRequest(
            "webhook_secret must be 16 to 256 characters".into(),
        ));
    }
    let id = db::provider_id(&s.pool, p.as_str())
        .await?
        .unwrap_or_else(Uuid::now_v7);
    let stored = s.vault.seal_col(
        "billing_providers",
        "webhook_secret",
        id,
        &req.webhook_secret,
    );
    let sealed = s.vault.ring().is_some();
    let mut tx = s.pool.begin().await?;
    if !db::upsert_provider_secret(&mut *tx, id, p.as_str(), &stored, sealed).await? {
        return Err(AppError::Conflict("provider changed concurrently, retry"));
    }
    audit::record(
        &mut *tx,
        &auth.principal,
        "billing.provider_secret",
        "billing_provider",
        id,
        Some(json!({"provider": p.as_str()})),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(ProviderView {
        provider: p.as_str().into(),
        webhook_secret_set: true,
    }))
}

#[derive(Serialize, ToSchema)]
pub struct WebhookAck {
    /// `applied`, `duplicate`, or `ignored` (stale event or unknown workspace).
    result: &'static str,
}

/// Provider subscription webhook. Public; authenticated by the HMAC of the raw body.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks/billing/{provider}",
    tag = "billing",
    params(("provider" = String, Path, description = "mock or lemonsqueezy")),
    responses(
        (status = 200, body = WebhookAck),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ),
)]
pub async fn webhook(
    State(s): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<WebhookAck>), AppError> {
    let p = Provider::parse(&provider).ok_or(AppError::NotFound("unknown provider"))?;
    match p {
        Provider::Mock => handle_webhook(&s, &MockProvider, &headers, &body).await,
        Provider::LemonSqueezy => {
            handle_webhook(&s, &LemonSqueezy::from_env(), &headers, &body).await
        }
    }
}

async fn handle_webhook<P: BillingProvider>(
    s: &AppState,
    p: &P,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(StatusCode, Json<WebhookAck>), AppError> {
    let name = p.kind().as_str();
    let (id, stored) = db::provider_secret(&s.pool, name)
        .await?
        .ok_or(AppError::NotFound("unknown provider"))?;
    let secret = s
        .vault
        .open_col("billing_providers", "webhook_secret", id, &stored)
        .map_err(|e| {
            tracing::error!(error = %e, "cannot open billing webhook secret");
            AppError::Internal
        })?;
    let sig = headers
        .get(p.signature_header())
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify(&secret, sig, body) {
        return Err(AppError::BadRequest("bad signature".into()));
    }
    let ev = p.parse_event(body).map_err(provider_err)?;
    let mut tx = s.pool.begin().await?;
    if !db::claim_event(&mut *tx, name, &ev.event_id).await? {
        return Ok((
            StatusCode::OK,
            Json(WebhookAck {
                result: "duplicate",
            }),
        ));
    }
    let applied = db::apply_event(
        &mut *tx,
        &db::EventUpdate {
            workspace_id: ev.workspace_id,
            provider: name,
            provider_subscription_id: ev.provider_subscription_id.as_deref(),
            status: ev.status.as_str(),
            plan_key: ev.plan_key.as_deref(),
            event_at: ev.updated_at,
            current_period_end: ev.current_period_end,
        },
    )
    .await?;
    tx.commit().await?;
    let result = if applied { "applied" } else { "ignored" };
    Ok((StatusCode::OK, Json(WebhookAck { result })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(extra_meta: &str, status: &str) -> Vec<u8> {
        format!(
            r#"{{"meta":{{"event_name":"subscription_updated",{extra_meta}"custom_data":{{"workspace_id":"0190a000-0000-7000-8000-000000000001","plan_key":"pro"}}}},
"data":{{"id":"77","attributes":{{"status":"{status}","updated_at":"2026-01-02T03:04:05.000000Z","renews_at":"2026-02-02T03:04:05.000000Z"}}}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn parses_lemonsqueezy_event() {
        let e = parse_lemonsqueezy(&body(r#""webhook_id":"wh_1","#, "active")).unwrap();
        assert_eq!(e.event_id, "wh_1");
        assert_eq!(e.status, SubStatus::Active);
        assert_eq!(e.plan_key.as_deref(), Some("pro"));
        assert_eq!(e.provider_subscription_id.as_deref(), Some("77"));
        assert!(e.current_period_end.is_some());
    }

    #[test]
    fn missing_event_id_falls_back_to_body_hash() {
        let a = parse_lemonsqueezy(&body("", "active")).unwrap();
        let b = parse_lemonsqueezy(&body("", "active")).unwrap();
        let c = parse_lemonsqueezy(&body("", "cancelled")).unwrap();
        assert_eq!(a.event_id, b.event_id);
        assert_ne!(a.event_id, c.event_id);
        assert_eq!(a.event_id.len(), 64);
    }

    #[test]
    fn rejects_bad_events() {
        assert!(parse_lemonsqueezy(b"nope").is_err());
        assert!(parse_lemonsqueezy(&body("", "mystery")).is_err());
        let no_ws = br#"{"meta":{"event_name":"subscription_created","custom_data":{}},"data":{"attributes":{"status":"active","updated_at":"2026-01-02T03:04:05Z"}}}"#;
        assert!(parse_lemonsqueezy(no_ws).is_err());
        let order = br#"{"meta":{"event_name":"order_created"},"data":{}}"#;
        assert!(parse_lemonsqueezy(order).is_err());
    }
}
