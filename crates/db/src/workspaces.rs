//! Workspaces and their admin members.

use serde::Serialize;
use sqlx::{PgExecutor, PgPool};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

pub async fn list_workspaces(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Workspace>, i64), sqlx::Error> {
    let items =
        sqlx::query_as("SELECT id, name, slug FROM workspaces ORDER BY id LIMIT $1 OFFSET $2")
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?;
    let total = sqlx::query_scalar("SELECT count(*) FROM workspaces")
        .fetch_one(pool)
        .await?;
    Ok((items, total))
}

pub async fn workspace_id_by_slug(
    db: impl PgExecutor<'_>,
    slug: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM workspaces WHERE slug = $1")
        .bind(slug)
        .fetch_optional(db)
        .await
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct Member {
    pub id: Uuid,
    pub email: String,
    pub role: String,
}

/// Admin-side users (`admin`, `super_admin`) of one workspace.
pub async fn list_admins(
    pool: &PgPool,
    workspace_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Member>, i64), sqlx::Error> {
    let items = sqlx::query_as(
        "SELECT id, email::text AS email, role::text AS role FROM users \
         WHERE workspace_id = $1 AND role <> 'affiliate' ORDER BY id LIMIT $2 OFFSET $3",
    )
    .bind(workspace_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE workspace_id = $1 AND role <> 'affiliate'",
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await?;
    Ok((items, total))
}
