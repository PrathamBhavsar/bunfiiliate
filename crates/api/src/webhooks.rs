//! Admin routes for outbound webhook endpoints and their delivery log.
//! Delivery itself lives in [`crate::dispatch`].

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_db::ops::{WebhookDelivery, WebhookEndpoint};
use serde::Deserialize;
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Auth, random_hex};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::page::{PageQuery, Paged};

const MAX_URL: usize = 2048;
const MAX_EVENTS: usize = 20;
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
}

fn endpoint_not_found() -> AppError {
    AppError::NotFound("webhook endpoint not found")
}

fn check_url(url: &str) -> Result<(), AppError> {
    let ok = url.len() <= MAX_URL
        && reqwest::Url::parse(url)
            .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some());
    if ok {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "url must be an absolute http(s) url".into(),
        ))
    }
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
    check_url(&req.url)?;
    check_events(&req.events)?;
    let e = WebhookEndpoint {
        id: Uuid::now_v7(),
        workspace_id: ws,
        url: req.url,
        secret: format!("whsec_{}", random_hex(24)),
        events: req.events,
        active: true,
    };
    let mut tx = s.pool.begin().await?;
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
    Ok(Json(page.wrap(rows)))
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
    Ok(Json(e.ok_or_else(endpoint_not_found)?))
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
        check_url(url)?;
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
    Ok(Json(e))
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
    bf_db::ops::requeue_delivery(&mut *tx, id).await?;
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
