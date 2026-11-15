//! Admin routes for outbound webhook endpoints and their delivery log.
//! Delivery itself lives in [`crate::dispatch`].

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_db::ops::{InboundEventRow, WebhookDelivery, WebhookEndpoint};
use serde::Deserialize;
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Auth, random_hex};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::page::{PageQuery, Paged};
use crate::ssrf;

const MAX_URL: usize = 2048;
const MAX_EVENTS: usize = 20;
/// Endpoints per workspace; each one gets a delivery row for every event.
const MAX_ENDPOINTS: i64 = 20;
const MAX_EVENT_LEN: usize = 64;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/webhooks/endpoints",
            get(list_endpoints).post(create_endpoint),
        )
        .route(
            "/api/v1/webhooks/endpoints/{id}",
            get(get_endpoint)
                .patch(update_endpoint)
                .delete(delete_endpoint),
        )
        .route("/api/v1/webhooks/deliveries", get(list_deliveries))
        .route(
            "/api/v1/webhooks/deliveries/{id}/redeliver",
            post(redeliver),
        )
        .route("/api/v1/webhooks/inbound", get(list_inbound))
        .route("/api/v1/webhooks/inbound/{id}/replay", post(replay_inbound))
        .route("/api/v1/webhooks/test", post(test_webhook))
}

fn endpoint_not_found() -> AppError {
    AppError::NotFound("webhook endpoint not found")
}

async fn check_url(url: &str) -> Result<(), AppError> {
    let ok = url.len() <= MAX_URL
        && reqwest::Url::parse(url)
            .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some());
    if !ok {
        return Err(AppError::BadRequest(
            "url must be an absolute http(s) url".into(),
        ));
    }
    ssrf::check_url(url).await
}

/// The stored secret is a ciphertext; it is never returned after creation.
fn redact(mut e: WebhookEndpoint) -> WebhookEndpoint {
    e.secret.clear();
    e
}

fn check_events(events: &[String]) -> Result<(), AppError> {
    if events.len() > MAX_EVENTS
        || events
            .iter()
            .any(|e| e.is_empty() || e.len() > MAX_EVENT_LEN)
    {
        return Err(AppError::BadRequest("invalid events".into()));
    }
    Ok(())
}

#[derive(Deserialize, ToSchema)]
pub struct NewEndpoint {
    url: String,
    /// Topics to receive (`conversion.created`, `conversion.approved`, ...); empty or omitted = all.
    #[serde(default)]
    events: Vec<String>,
}

/// Register an endpoint. Deliveries are POSTed as JSON with
/// `X-Bf-Signature: t=<unix>,v1=<hex HMAC-SHA256 of "<t>.<body>" keyed by the secret>`.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks/endpoints",
    tag = "webhooks",
    security(("cookie" = [])),
    request_body = NewEndpoint,
    responses((status = 201, body = WebhookEndpoint), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody))
)]
pub async fn create_endpoint(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<NewEndpoint>,
) -> Result<(StatusCode, Json<WebhookEndpoint>), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    check_url(&req.url).await?;
    check_events(&req.events)?;
    let id = Uuid::now_v7();
    let plain_secret = format!("whsec_{}", random_hex(24));
    let mut e = WebhookEndpoint {
        id,
        workspace_id: ws,
        url: req.url,
        secret: s
            .vault
            .seal_col("webhook_endpoints", "secret", id, &plain_secret),
        events: req.events,
        active: true,
    };
    let mut tx = s.pool.begin().await?;
    let have: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webhook_endpoints WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&mut *tx)
            .await?;
    if have >= MAX_ENDPOINTS {
        return Err(AppError::BadRequest(
            "a workspace can have at most 20 webhook endpoints".into(),
        ));
    }
    bf_db::ops::insert_endpoint(&mut *tx, &e).await?;
    let diff = json!({ "url": e.url, "events": e.events });
    let action = "webhook_endpoint.create";
    audit::record(
        &mut *tx,
        &auth.principal,
        action,
        "webhook_endpoint",
        e.id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    // Shown once: the response carries the plaintext, never the stored value.
    e.secret = plain_secret;
    Ok((StatusCode::CREATED, Json(e)))
}

/// List endpoints.
#[utoipa::path(
    get,
    path = "/api/v1/webhooks/endpoints",
    tag = "webhooks",
    security(("cookie" = [])),
    params(PageQuery),
    responses((status = 200, body = Paged<WebhookEndpoint>), (status = 403, body = ErrorBody))
)]
pub async fn list_endpoints(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<WebhookEndpoint>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rows = bf_db::ops::list_endpoints(&s.pool, ws, page.limit(), page.offset()).await?;
    let (items, total) = rows;
    Ok(Json(
        page.wrap((items.into_iter().map(redact).collect(), total)),
    ))
}

/// One endpoint.
#[utoipa::path(
    get,
    path = "/api/v1/webhooks/endpoints/{id}",
    tag = "webhooks",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Endpoint id")),
    responses((status = 200, body = WebhookEndpoint), (status = 404, body = ErrorBody))
)]
pub async fn get_endpoint(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<WebhookEndpoint>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let e = bf_db::ops::get_endpoint(&s.pool, ws, id).await?;
    Ok(Json(redact(e.ok_or_else(endpoint_not_found)?)))
}

#[derive(Deserialize, ToSchema)]
pub struct EndpointPatch {
    url: Option<String>,
    events: Option<Vec<String>>,
    /// `false` stops new and pending deliveries until switched back on.
    active: Option<bool>,
}

/// Change an endpoint's url, topics or active flag.
#[utoipa::path(
    patch,
    path = "/api/v1/webhooks/endpoints/{id}",
    tag = "webhooks",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Endpoint id")),
    request_body = EndpointPatch,
    responses((status = 200, body = WebhookEndpoint), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn update_endpoint(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<EndpointPatch>,
) -> Result<Json<WebhookEndpoint>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    if let Some(url) = &req.url {
        check_url(url).await?;
    }
    if let Some(events) = &req.events {
        check_events(events)?;
    }
    let mut tx = s.pool.begin().await?;
    let e = bf_db::ops::update_endpoint(
        &mut *tx,
        ws,
        id,
        req.url.as_deref(),
        req.events.as_deref(),
        req.active,
    )
    .await?
    .ok_or_else(endpoint_not_found)?;
    let diff = json!({ "url": req.url, "events": req.events, "active": req.active });
    let action = "webhook_endpoint.update";
    audit::record(
        &mut *tx,
        &auth.principal,
        action,
        "webhook_endpoint",
        id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(redact(e)))
}

/// Delete an endpoint and its delivery log.
#[utoipa::path(
    delete,
    path = "/api/v1/webhooks/endpoints/{id}",
    tag = "webhooks",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Endpoint id")),
    responses((status = 204), (status = 404, body = ErrorBody))
)]
pub async fn delete_endpoint(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    if !bf_db::ops::delete_endpoint(&mut *tx, ws, id).await? {
        return Err(endpoint_not_found());
    }
    let action = "webhook_endpoint.delete";
    audit::record(
        &mut *tx,
        &auth.principal,
        action,
        "webhook_endpoint",
        id,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeliveryFilter {
    endpoint_id: Option<Uuid>,
}

/// Delivery log, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/webhooks/deliveries",
    tag = "webhooks",
    security(("cookie" = [])),
    params(DeliveryFilter, PageQuery),
    responses((status = 200, body = Paged<WebhookDelivery>), (status = 403, body = ErrorBody))
)]
pub async fn list_deliveries(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<DeliveryFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<WebhookDelivery>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rows = bf_db::ops::list_deliveries(&s.pool, ws, f.endpoint_id, page.limit(), page.offset())
        .await?;
    Ok(Json(page.wrap(rows)))
}

/// Send a finished (delivered or dead) delivery again, with a fresh set of attempts.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks/deliveries/{id}/redeliver",
    tag = "webhooks",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Delivery id")),
    responses(
        (status = 200, body = WebhookDelivery),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Still being retried", body = ErrorBody),
    )
)]
pub async fn redeliver(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<WebhookDelivery>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    let d = bf_db::ops::get_delivery(&mut *tx, ws, id)
        .await?
        .ok_or(AppError::NotFound("delivery not found"))?;
    if d.next_attempt_at.is_some() {
        return Err(AppError::Conflict("delivery is still pending"));
    }
    bf_db::ops::requeue_delivery(&mut *tx, ws, id).await?;
    let action = "webhook_delivery.redeliver";
    audit::record(
        &mut *tx,
        &auth.principal,
        action,
        "webhook_delivery",
        id,
        None,
    )
    .await?;
    let d = bf_db::ops::get_delivery(&mut *tx, ws, id)
        .await?
        .ok_or(AppError::Internal)?;
    tx.commit().await?;
    Ok(Json(d))
}

// ---- inbound events: history, replay and the signed test sender

const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Largest slice of the tracker's response returned to the caller.
const MAX_TEST_RESPONSE: usize = 4096;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct InboundFilter {
    program_id: Option<Uuid>,
    /// `generic`, `stripe` or `paddle`.
    source: Option<String>,
}

/// Received deliveries (newest first) with their processing outcome.
#[utoipa::path(
    get,
    path = "/api/v1/webhooks/inbound",
    tag = "webhooks",
    security(("cookie" = [])),
    params(InboundFilter, PageQuery),
    responses((status = 200, body = Paged<InboundEventRow>), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody))
)]
pub async fn list_inbound(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<InboundFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<InboundEventRow>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    if f.source
        .as_deref()
        .is_some_and(|x| !matches!(x, "generic" | "stripe" | "paddle"))
    {
        return Err(AppError::BadRequest("unknown source".into()));
    }
    let rows = bf_db::ops::list_inbound(
        &s.pool,
        ws,
        f.program_id,
        f.source.as_deref(),
        page.limit(),
        page.offset(),
    )
    .await?;
    Ok(Json(page.wrap(rows)))
}

/// Run a stored delivery through the worker again. The signature is not re-checked: it
/// passed when the event was stored. Duplicate sales still dedupe on the external id.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks/inbound/{id}/replay",
    tag = "webhooks",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Inbound event id")),
    responses(
        (status = 200, body = InboundEventRow),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Still queued", body = ErrorBody),
    )
)]
pub async fn replay_inbound(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<InboundEventRow>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    let ev = bf_db::ops::get_inbound(&mut *tx, ws, id)
        .await?
        .ok_or(AppError::NotFound("inbound event not found"))?;
    if ev.processed_at.is_none() {
        return Err(AppError::Conflict("event is still queued"));
    }
    bf_db::ops::requeue_inbound(&mut *tx, ws, id).await?;
    audit::record(
        &mut *tx,
        &auth.principal,
        "webhook_inbound.replay",
        "inbound_event",
        id,
        None,
    )
    .await?;
    let ev = bf_db::ops::get_inbound(&mut *tx, ws, id)
        .await?
        .ok_or(AppError::Internal)?;
    tx.commit().await?;
    Ok(Json(ev))
}

#[derive(Deserialize, ToSchema)]
pub struct TestWebhook {
    program_id: Uuid,
    /// `generic`, `stripe` or `paddle`.
    source: String,
    /// The event body to deliver.
    payload: serde_json::Value,
}

#[derive(serde::Serialize, ToSchema)]
pub struct TestWebhookResult {
    /// The tracker's HTTP status.
    status: u16,
    /// The tracker's response body (first 4 KiB).
    body: String,
}

/// Sign `payload` with the program's own secret for `source` and deliver it to the tracker's
/// real inbound endpoint, returning the tracker's raw answer. Lets an admin check a secret
/// end to end without a provider account.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks/test",
    tag = "webhooks",
    security(("cookie" = [])),
    request_body = TestWebhook,
    responses(
        (status = 200, body = TestWebhookResult),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 502, description = "Tracker unreachable", body = ErrorBody),
    )
)]
pub async fn test_webhook(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<TestWebhook>,
) -> Result<Json<TestWebhookResult>, AppError> {
    use bf_core::webhook_sign::{Layout, sign};
    let ws = auth.principal.workspace_scope(None)?;
    let (layout, path, column) = match req.source.as_str() {
        "generic" => (Layout::Generic, "v1/events", "webhook_secret"),
        "stripe" => (Layout::Stripe, "v1/webhooks/stripe", "stripe_secret"),
        "paddle" => (Layout::Paddle, "v1/webhooks/paddle", "paddle_secret"),
        _ => return Err(AppError::BadRequest("unknown source".into())),
    };
    let program = bf_db::programs::get_program(&s.pool, ws, req.program_id, None)
        .await?
        .ok_or(AppError::NotFound("program not found"))?;
    let stored = match column {
        "webhook_secret" => program.webhook_secret.clone(),
        "stripe_secret" => program.stripe_secret.clone(),
        _ => program.paddle_secret.clone(),
    }
    .filter(|v| !v.is_empty())
    .ok_or_else(|| AppError::BadRequest(format!("program has no {} secret set", req.source)))?;
    let secret = s
        .vault
        .open_col("programs", column, program.id, &stored)
        .map_err(|_| AppError::Internal)?;
    let body = serde_json::to_vec(&req.payload).map_err(|_| AppError::Internal)?;
    let key = format!("test-{}", Uuid::now_v7());
    let headers = sign(layout, &secret, &key, &body, chrono::Utc::now().timestamp());
    let url = format!("{}/{path}/{}", s.tracker_url.0, program.public_key);
    let mut http = reqwest::Client::builder()
        .timeout(TEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AppError::Internal)?
        .post(url)
        .header("content-type", "application/json")
        .body(body);
    for (k, v) in headers {
        http = http.header(k, v);
    }
    let resp = http.send().await.map_err(|e| {
        tracing::warn!(error = %e, "webhook test: tracker unreachable");
        AppError::BadGateway
    })?;
    let status = resp.status().as_u16();
    let mut text = resp.text().await.unwrap_or_default();
    text.truncate(text.floor_char_boundary(MAX_TEST_RESPONSE));
    audit::record(
        &s.pool,
        &auth.principal,
        "webhook_inbound.test",
        "program",
        program.id,
        Some(json!({ "source": req.source, "status": status })),
    )
    .await?;
    Ok(Json(TestWebhookResult { status, body: text }))
}
