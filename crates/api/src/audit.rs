//! Audit-log helper: who did what to which entity.

use std::fmt::Display;

use bf_db::auth::{AuditEntry, insert_audit};
use sqlx::PgExecutor;

use crate::auth::Principal;
use crate::error::AppError;

pub async fn record(
    db: impl PgExecutor<'_>,
    actor: &Principal,
    action: &str,
    entity: &str,
    entity_id: impl Display,
    diff: Option<serde_json::Value>,
) -> Result<(), AppError> {
    let diff = diff.map(|d| d.to_string());
    let entry = AuditEntry {
        workspace_id: actor.workspace_id,
        actor_id: Some(actor.user_id),
        action,
        entity,
        entity_id: &entity_id.to_string(),
        diff: diff.as_deref(),
    };
    Ok(insert_audit(db, &entry).await?)
}
