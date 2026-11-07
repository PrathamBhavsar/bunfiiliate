//! Smaller ops routes: notifications (own user), contact requests, workspace settings,
//! audit log.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_db::auth::{AuditEntry, insert_audit};
use bf_db::ops::{AuditRecord, ContactRequest, Notification};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Any, Auth};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::limit;
use crate::page::{PageQuery, Paged};

const MAX_NAME: usize = 200;
const MAX_MESSAGE: usize = 5000;
const MAX_SETTINGS: usize = 16 * 1024;
const CONTACT_STATUSES: [&str; 3] = ["new", "read", "closed"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/v1/notifications", get(list_notifications))
        .route("/api/v1/notifications/{id}/read", post(mark_read))
        .route(
            "/api/v1/contact-requests",
            get(list_contacts).post(create_contact),
        )
        .route(
            "/api/v1/contact-requests/{id}",
            axum::routing::patch(update_contact),
        )
        .route("/api/v1/settings", get(get_settings).put(put_settings))
        .route("/api/v1/audit-log", get(list_audit))
}

// ---- health

/// Liveness: the process is up. No auth, no DB.
#[utoipa::path(get, path = "/healthz", tag = "ops", responses((status = 200, description = "alive")))]
pub async fn healthz() -> &'static str {
    "ok"
}

/// Readiness: Postgres answers and the schema is at (or past) the migration head
/// this binary embeds. No auth.
#[utoipa::path(
    get,
    path = "/readyz",
    tag = "ops",
    responses((status = 200, description = "ready"), (status = 503, description = "not ready"))
)]
pub async fn readyz(State(s): State<AppState>) -> (StatusCode, Json<Value>) {
    let want = bf_db::embedded_head();
    match bf_db::applied_head(&s.pool).await {
        Ok(have) if have >= want => (
            StatusCode::OK,
            Json(json!({"status": "ok", "migration_head": have})),
        ),
        Ok(have) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "migrating", "migration_head": have, "expected": want})),
        ),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "db_unavailable"})),
        ),
    }
}

// ---- notifications

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NotificationFilter {
    /// Only unread notifications.
    #[serde(default)]
    unread: bool,
}

/// The signed-in user's notifications, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/notifications",
    tag = "notifications",
    security(("cookie" = [])),
    params(NotificationFilter, PageQuery),
    responses((status = 200, body = Paged<Notification>), (status = 401, body = ErrorBody))
)]
pub async fn list_notifications(
    State(s): State<AppState>,
    auth: Auth<Any>,
    Query(f): Query<NotificationFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Notification>>, AppError> {
    let user = auth.principal.user_id;
    let rows = bf_db::ops::list_notifications(&s.pool, user, f.unread, page.limit(), page.offset())
        .await?;
    Ok(Json(page.wrap(rows)))
}

/// Mark one of the signed-in user's notifications read.
#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/read",
    tag = "notifications",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Notification id")),
    responses((status = 200, body = Notification), (status = 404, body = ErrorBody))
)]
pub async fn mark_read(
    State(s): State<AppState>,
    auth: Auth<Any>,
    Path(id): Path<Uuid>,
) -> Result<Json<Notification>, AppError> {
    let n = bf_db::ops::mark_notification_read(&s.pool, auth.principal.user_id, id).await?;
    Ok(Json(n.ok_or(AppError::NotFound("notification not found"))?))
}

// ---- contact requests

#[derive(Deserialize, ToSchema)]
pub struct NewContact {
    /// Slug of the workspace to write to.
    workspace: String,
    program_id: Option<Uuid>,
    name: String,
    email: String,
    message: String,
}

/// Public contact form: stores a `new` request for the workspace's admins.
#[utoipa::path(
    post,
    path = "/api/v1/contact-requests",
    tag = "contact-requests",
    request_body = NewContact,
    responses(
        (status = 201, body = ContactRequest),
        (status = 400, body = ErrorBody),
        (status = 404, description = "Unknown workspace", body = ErrorBody),
    )
)]
pub async fn create_contact(
    State(s): State<AppState>,
    headers: HeaderMap,
    AppJson(req): AppJson<NewContact>,
) -> Result<(StatusCode, Json<ContactRequest>), AppError> {
    limit::check(&s.limiter, &limit::CONTACT, &headers, &req.email)?;
    let (name, email, message) = (req.name.trim(), req.email.trim(), req.message.trim());
    if name.is_empty() || name.len() > MAX_NAME {
        return Err(AppError::BadRequest("name must be 1-200 characters".into()));
    }
    if !email.contains('@') || email.len() > MAX_NAME {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    if message.is_empty() || message.len() > MAX_MESSAGE {
        return Err(AppError::BadRequest(
            "message must be 1-5000 characters".into(),
        ));
    }
    let ws = bf_db::workspaces::workspace_id_by_slug(&s.pool, &req.workspace)
        .await?
        .ok_or(AppError::NotFound("workspace not found"))?;
    let id = Uuid::now_v7();
    let mut tx = s.pool.begin().await?;
    let c = bf_db::ops::insert_contact(&mut *tx, id, ws, req.program_id, name, email, message)
        .await?
        .ok_or_else(|| AppError::BadRequest("unknown program".into()))?;
    let diff = json!({ "program_id": c.program_id, "email": c.email }).to_string();
    let entry = AuditEntry {
        workspace_id: Some(ws),
        actor_id: None,
        action: "contact_request.create",
        entity: "contact_request",
        entity_id: &id.to_string(),
        diff: Some(&diff),
    };
    insert_audit(&mut *tx, &entry).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(c)))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ContactFilter {
    /// `new`, `read` or `closed`.
    status: Option<String>,
}

/// List contact requests, newest first, optionally by status.
#[utoipa::path(
    get,
    path = "/api/v1/contact-requests",
    tag = "contact-requests",
    security(("cookie" = [])),
    params(ContactFilter, PageQuery),
    responses((status = 200, body = Paged<ContactRequest>), (status = 403, body = ErrorBody))
)]
pub async fn list_contacts(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<ContactFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<ContactRequest>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rows = bf_db::ops::list_contacts(
        &s.pool,
        ws,
        f.status.as_deref(),
        page.limit(),
        page.offset(),
    )
    .await?;
    Ok(Json(page.wrap(rows)))
}

#[derive(Deserialize, ToSchema)]
pub struct ContactPatch {
    /// `new`, `read` or `closed`.
    status: String,
}

/// Set a contact request's status.
#[utoipa::path(
    patch,
    path = "/api/v1/contact-requests/{id}",
    tag = "contact-requests",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Contact request id")),
    request_body = ContactPatch,
    responses((status = 200, body = ContactRequest), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn update_contact(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<ContactPatch>,
) -> Result<Json<ContactRequest>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    if !CONTACT_STATUSES.contains(&req.status.as_str()) {
        return Err(AppError::BadRequest(format!(
            "status must be one of: {}",
            CONTACT_STATUSES.join(", ")
        )));
    }
    let mut tx = s.pool.begin().await?;
    let c = bf_db::ops::set_contact_status(&mut *tx, ws, id, &req.status)
        .await?
        .ok_or(AppError::NotFound("contact request not found"))?;
    let diff = json!({ "status": c.status });
    let action = "contact_request.update";
    audit::record(
        &mut *tx,
        &auth.principal,
        action,
        "contact_request",
        id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(c))
}

// ---- settings

/// The workspace's settings object.
#[utoipa::path(
    get,
    path = "/api/v1/settings",
    tag = "settings",
    security(("cookie" = [])),
    responses((status = 200, description = "Settings object", body = Object), (status = 403, body = ErrorBody))
)]
pub async fn get_settings(
    State(s): State<AppState>,
    auth: Auth<Admin>,
) -> Result<Json<Value>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let settings = bf_db::ops::get_settings(&s.pool, ws).await?;
    Ok(Json(
        settings.ok_or(AppError::NotFound("workspace not found"))?,
    ))
}

/// Replace the workspace's settings with the given JSON object (max 16 KiB).
#[utoipa::path(
    put,
    path = "/api/v1/settings",
    tag = "settings",
    security(("cookie" = [])),
    request_body(content = Object, description = "New settings object"),
    responses((status = 200, description = "Stored settings", body = Object), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody))
)]
pub async fn put_settings(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(settings): AppJson<Value>,
) -> Result<Json<Value>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let Some(obj) = settings.as_object() else {
        return Err(AppError::BadRequest(
            "settings must be a JSON object".into(),
        ));
    };
    if settings.to_string().len() > MAX_SETTINGS {
        return Err(AppError::BadRequest("settings too large".into()));
    }
    let mut tx = s.pool.begin().await?;
    if !bf_db::ops::set_settings(&mut *tx, ws, &settings).await? {
        return Err(AppError::NotFound("workspace not found"));
    }
    // Values may be sensitive: the audit entry records only which keys are set.
    let diff = json!({ "keys": obj.keys().collect::<Vec<_>>() });
    audit::record(
        &mut *tx,
        &auth.principal,
        "settings.update",
        "workspace",
        ws,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(settings))
}

// ---- audit log

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditFilter {
    /// Entity type, e.g. `program`.
    entity: Option<String>,
    /// Action, e.g. `program.create`.
    action: Option<String>,
}

/// The workspace's audit log, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/audit-log",
    tag = "audit-log",
    security(("cookie" = [])),
    params(AuditFilter, PageQuery),
    responses((status = 200, body = Paged<AuditRecord>), (status = 403, body = ErrorBody))
)]
pub async fn list_audit(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<AuditFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<AuditRecord>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rows = bf_db::ops::list_audit(
        &s.pool,
        ws,
        f.entity.as_deref(),
        f.action.as_deref(),
        page.limit(),
        page.offset(),
    )
    .await?;
    Ok(Json(page.wrap(rows)))
}
