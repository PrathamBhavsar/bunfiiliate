//! Ops tables: webhook endpoints and deliveries, email templates, notifications,
//! contact requests, workspace settings, audit log. Every query is scoped to a workspace
//! (notifications to a user).

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgExecutor, PgPool};
use utoipa::ToSchema;
use uuid::Uuid;

// ---- webhook endpoints

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct WebhookEndpoint {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub url: String,
    /// Signing secret; only admins can read it.
    pub secret: String,
    /// Topics to deliver; empty means every topic.
    pub events: Vec<String>,
    pub active: bool,
}

const ENDPOINT_COLS: &str = "id, workspace_id, url, secret, events, active";

pub async fn insert_endpoint(
    db: impl PgExecutor<'_>,
    e: &WebhookEndpoint,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO webhook_endpoints (id, workspace_id, url, secret, events, active) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(e.id)
    .bind(e.workspace_id)
    .bind(&e.url)
    .bind(&e.secret)
    .bind(&e.events)
    .bind(e.active)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn list_endpoints(
    pool: &PgPool,
    workspace_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(Vec<WebhookEndpoint>, i64), sqlx::Error> {
    let items = sqlx::query_as(&format!(
        "SELECT {ENDPOINT_COLS} FROM webhook_endpoints WHERE workspace_id = $1 \
         ORDER BY id LIMIT $2 OFFSET $3"
    ))
    .bind(workspace_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total =
        sqlx::query_scalar("SELECT count(*) FROM webhook_endpoints WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(pool)
            .await?;
    Ok((items, total))
}

pub async fn get_endpoint(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<Option<WebhookEndpoint>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {ENDPOINT_COLS} FROM webhook_endpoints WHERE id = $1 AND workspace_id = $2"
    ))
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(db)
    .await
}

/// Partial update: `None` keeps the stored value.
pub async fn update_endpoint(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
    url: Option<&str>,
    events: Option<&[String]>,
    active: Option<bool>,
) -> Result<Option<WebhookEndpoint>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE webhook_endpoints SET url = COALESCE($3, url), events = COALESCE($4, events), \
         active = COALESCE($5, active) WHERE id = $1 AND workspace_id = $2 \
         RETURNING {ENDPOINT_COLS}"
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(url)
    .bind(events)
    .bind(active)
    .fetch_optional(db)
    .await
}

pub async fn delete_endpoint(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let n = sqlx::query("DELETE FROM webhook_endpoints WHERE id = $1 AND workspace_id = $2")
        .bind(id)
        .bind(workspace_id)
        .execute(db)
        .await?
        .rows_affected();
    Ok(n > 0)
}

// ---- webhook deliveries

/// One endpoint x outbox row. `next_attempt_at` set = still pending; unset with a 2xx
/// `status_code` = delivered; unset otherwise = dead (attempts exhausted).
#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct WebhookDelivery {
    pub id: Uuid,
    pub endpoint_id: Uuid,
    pub outbox_id: i64,
    pub topic: String,
    /// Attempts made so far.
    pub attempt: i32,
    /// Last HTTP status; null before the first attempt or after a network error.
    pub status_code: Option<i32>,
    pub next_attempt_at: Option<DateTime<Utc>>,
}

const DELIVERY_SELECT: &str = "SELECT d.id, d.endpoint_id, d.outbox_id, o.topic, d.attempt, \
     d.status_code, d.next_attempt_at FROM webhook_deliveries d \
     JOIN webhook_endpoints e ON e.id = d.endpoint_id JOIN outbox o ON o.id = d.outbox_id";

pub async fn list_deliveries(
    pool: &PgPool,
    workspace_id: Uuid,
    endpoint_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<WebhookDelivery>, i64), sqlx::Error> {
    const FILTER: &str = "e.workspace_id = $1 AND ($2::uuid IS NULL OR d.endpoint_id = $2)";
    let items = sqlx::query_as(&format!(
        "{DELIVERY_SELECT} WHERE {FILTER} ORDER BY d.id DESC LIMIT $3 OFFSET $4"
    ))
    .bind(workspace_id)
    .bind(endpoint_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM webhook_deliveries d \
         JOIN webhook_endpoints e ON e.id = d.endpoint_id WHERE {FILTER}"
    ))
    .bind(workspace_id)
    .bind(endpoint_id)
    .fetch_one(pool)
    .await?;
    Ok((items, total))
}

pub async fn get_delivery(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<Option<WebhookDelivery>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{DELIVERY_SELECT} WHERE d.id = $1 AND e.workspace_id = $2"
    ))
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(db)
    .await
}

/// Queue a finished delivery for a fresh run of attempts.
pub async fn requeue_delivery(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE webhook_deliveries d SET attempt = 0, next_attempt_at = now() \
         FROM webhook_endpoints e \
         WHERE e.id = d.endpoint_id AND e.workspace_id = $2 AND d.id = $1",
    )
    .bind(id)
    .bind(workspace_id)
    .execute(db)
    .await?;
    Ok(())
}

// ---- email templates

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct EmailTemplate {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub key: String,
    pub subject: String,
    pub body: String,
}

const TEMPLATE_COLS: &str = "id, workspace_id, key, subject, body";

pub async fn insert_template(
    db: impl PgExecutor<'_>,
    t: &EmailTemplate,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO email_templates (id, workspace_id, key, subject, body) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(t.id)
    .bind(t.workspace_id)
    .bind(&t.key)
    .bind(&t.subject)
    .bind(&t.body)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn list_templates(
    pool: &PgPool,
    workspace_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(Vec<EmailTemplate>, i64), sqlx::Error> {
    let items = sqlx::query_as(&format!(
        "SELECT {TEMPLATE_COLS} FROM email_templates WHERE workspace_id = $1 \
         ORDER BY key LIMIT $2 OFFSET $3"
    ))
    .bind(workspace_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar("SELECT count(*) FROM email_templates WHERE workspace_id = $1")
        .bind(workspace_id)
        .fetch_one(pool)
        .await?;
    Ok((items, total))
}

pub async fn get_template(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<Option<EmailTemplate>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {TEMPLATE_COLS} FROM email_templates WHERE id = $1 AND workspace_id = $2"
    ))
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(db)
    .await
}

pub async fn template_by_key(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    key: &str,
) -> Result<Option<EmailTemplate>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {TEMPLATE_COLS} FROM email_templates WHERE workspace_id = $1 AND key = $2"
    ))
    .bind(workspace_id)
    .bind(key)
    .fetch_optional(db)
    .await
}

pub async fn update_template(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
    subject: Option<&str>,
    body: Option<&str>,
) -> Result<Option<EmailTemplate>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE email_templates SET subject = COALESCE($3, subject), body = COALESCE($4, body) \
         WHERE id = $1 AND workspace_id = $2 RETURNING {TEMPLATE_COLS}"
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(subject)
    .bind(body)
    .fetch_optional(db)
    .await
}

pub async fn delete_template(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let n = sqlx::query("DELETE FROM email_templates WHERE id = $1 AND workspace_id = $2")
        .bind(id)
        .bind(workspace_id)
        .execute(db)
        .await?
        .rows_affected();
    Ok(n > 0)
}

// ---- notifications

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct Notification {
    pub id: Uuid,
    pub kind: String,
    pub body: String,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn list_notifications(
    pool: &PgPool,
    user_id: Uuid,
    unread_only: bool,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Notification>, i64), sqlx::Error> {
    const FILTER: &str = "user_id = $1 AND (NOT $2 OR read_at IS NULL)";
    let items = sqlx::query_as(&format!(
        "SELECT id, kind, body, read_at, created_at FROM notifications WHERE {FILTER} \
         ORDER BY created_at DESC, id DESC LIMIT $3 OFFSET $4"
    ))
    .bind(user_id)
    .bind(unread_only)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM notifications WHERE {FILTER}"
    ))
    .bind(user_id)
    .bind(unread_only)
    .fetch_one(pool)
    .await?;
    Ok((items, total))
}

/// Mark read (idempotent: keeps the first read time).
pub async fn mark_notification_read(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
    id: Uuid,
) -> Result<Option<Notification>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE notifications SET read_at = COALESCE(read_at, now()) \
         WHERE id = $1 AND user_id = $2 RETURNING id, kind, body, read_at, created_at",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await
}

// ---- contact requests

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct ContactRequest {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub program_id: Option<Uuid>,
    pub name: String,
    pub email: String,
    pub message: String,
    /// `new`, `read` or `closed`.
    pub status: String,
    pub created_at: DateTime<Utc>,
}

const CONTACT_COLS: &str =
    "id, workspace_id, program_id, name, email::text AS email, message, status, created_at";

/// Inserts a `new` request; `program_id` must belong to the workspace.
pub async fn insert_contact(
    db: impl PgExecutor<'_>,
    id: Uuid,
    workspace_id: Uuid,
    program_id: Option<Uuid>,
    name: &str,
    email: &str,
    message: &str,
) -> Result<Option<ContactRequest>, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO contact_requests (id, workspace_id, program_id, name, email, message) \
         SELECT $1, $2, $3, $4, $5, $6 \
         WHERE $3::uuid IS NULL \
            OR EXISTS (SELECT 1 FROM programs WHERE id = $3 AND workspace_id = $2) \
         RETURNING {CONTACT_COLS}"
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(program_id)
    .bind(name)
    .bind(email)
    .bind(message)
    .fetch_optional(db)
    .await
}

pub async fn list_contacts(
    pool: &PgPool,
    workspace_id: Uuid,
    status: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<ContactRequest>, i64), sqlx::Error> {
    const FILTER: &str = "workspace_id = $1 AND ($2::text IS NULL OR status = $2)";
    let items = sqlx::query_as(&format!(
        "SELECT {CONTACT_COLS} FROM contact_requests WHERE {FILTER} \
         ORDER BY created_at DESC, id DESC LIMIT $3 OFFSET $4"
    ))
    .bind(workspace_id)
    .bind(status)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM contact_requests WHERE {FILTER}"
    ))
    .bind(workspace_id)
    .bind(status)
    .fetch_one(pool)
    .await?;
    Ok((items, total))
}

pub async fn set_contact_status(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
    status: &str,
) -> Result<Option<ContactRequest>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE contact_requests SET status = $3 WHERE id = $1 AND workspace_id = $2 \
         RETURNING {CONTACT_COLS}"
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(status)
    .fetch_optional(db)
    .await
}

// ---- workspace settings

pub async fn get_settings(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
) -> Result<Option<Value>, sqlx::Error> {
    sqlx::query_scalar("SELECT settings FROM workspaces WHERE id = $1")
        .bind(workspace_id)
        .fetch_optional(db)
        .await
}

pub async fn set_settings(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    settings: &Value,
) -> Result<bool, sqlx::Error> {
    let n = sqlx::query("UPDATE workspaces SET settings = $2 WHERE id = $1")
        .bind(workspace_id)
        .bind(settings)
        .execute(db)
        .await?
        .rows_affected();
    Ok(n > 0)
}

// ---- audit log

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct AuditRecord {
    pub id: Uuid,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub entity: String,
    pub entity_id: String,
    #[schema(value_type = Option<Object>)]
    pub diff: Option<Value>,
    pub at: DateTime<Utc>,
}

pub async fn list_audit(
    pool: &PgPool,
    workspace_id: Uuid,
    entity: Option<&str>,
    action: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<AuditRecord>, i64), sqlx::Error> {
    const FILTER: &str = "workspace_id = $1 AND ($2::text IS NULL OR entity = $2) AND ($3::text IS NULL OR action = $3)";
    let items = sqlx::query_as(&format!(
        "SELECT id, actor_id, action, entity, entity_id, diff, at FROM audit_log \
         WHERE {FILTER} ORDER BY at DESC, id DESC LIMIT $4 OFFSET $5"
    ))
    .bind(workspace_id)
    .bind(entity)
    .bind(action)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!("SELECT count(*) FROM audit_log WHERE {FILTER}"))
        .bind(workspace_id)
        .bind(entity)
        .bind(action)
        .fetch_one(pool)
        .await?;
    Ok((items, total))
}
