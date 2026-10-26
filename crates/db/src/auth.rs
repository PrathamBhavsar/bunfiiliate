//! Users, workspaces, refresh-token families and the audit log.

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, serde::Serialize, serde::Deserialize)]
#[sqlx(type_name = "user_role", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    SuperAdmin,
    Admin,
    Affiliate,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub email: String,
    pub password_hash: String,
    pub role: UserRole,
}

const USER_COLS: &str = "id, workspace_id, email::text AS email, password_hash, role";

pub async fn create_workspace(
    db: impl PgExecutor<'_>,
    id: Uuid,
    name: &str,
    slug: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(name)
        .bind(slug)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn create_user(db: impl PgExecutor<'_>, user: &User) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users (id, workspace_id, email, password_hash, role) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user.id)
    .bind(user.workspace_id)
    .bind(&user.email)
    .bind(&user.password_hash)
    .bind(user.role)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn super_admin_exists(db: impl PgExecutor<'_>) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE role = 'super_admin')")
        .fetch_one(db)
        .await
}

pub async fn user_by_email(
    db: impl PgExecutor<'_>,
    email: &str,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE email = $1"))
        .bind(email)
        .fetch_optional(db)
        .await
}

pub async fn user_by_id(db: impl PgExecutor<'_>, id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await
}

/// Start a new refresh-token family.
pub async fn issue_refresh(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    insert_refresh(
        db,
        Uuid::now_v7(),
        user_id,
        Uuid::now_v7(),
        token_hash,
        expires_at,
    )
    .await
}

async fn insert_refresh(
    db: impl PgExecutor<'_>,
    id: Uuid,
    user_id: Uuid,
    family_id: Uuid,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO refresh_tokens (id, user_id, family_id, token_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(user_id)
    .bind(family_id)
    .bind(token_hash)
    .bind(expires_at)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    /// Old token revoked, new one stored in the same family.
    Rotated { user_id: Uuid },
    /// An already-rotated or revoked token came back: the whole family is now revoked.
    Reuse,
    /// Unknown or expired token.
    Invalid,
}

/// Swap `old_hash` for `new_hash`. Presenting a token that was already used
/// revokes every token of its family.
pub async fn rotate_refresh(
    pool: &PgPool,
    old_hash: &[u8],
    new_hash: &[u8],
    new_expires_at: DateTime<Utc>,
) -> Result<Rotation, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row: Option<(Uuid, Uuid, Uuid, bool, bool)> = sqlx::query_as(
        "SELECT id, user_id, family_id, revoked_at IS NOT NULL, expires_at <= now() \
         FROM refresh_tokens WHERE token_hash = $1 FOR UPDATE",
    )
    .bind(old_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, user_id, family_id, revoked, expired)) = row else {
        return Ok(Rotation::Invalid);
    };
    if revoked {
        revoke_family(&mut tx, family_id).await?;
        tx.commit().await?;
        return Ok(Rotation::Reuse);
    }
    if expired {
        return Ok(Rotation::Invalid);
    }
    let new_id = Uuid::now_v7();
    insert_refresh(
        &mut *tx,
        new_id,
        user_id,
        family_id,
        new_hash,
        new_expires_at,
    )
    .await?;
    sqlx::query("UPDATE refresh_tokens SET revoked_at = now(), replaced_by = $2 WHERE id = $1")
        .bind(id)
        .bind(new_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Rotation::Rotated { user_id })
}

/// Revoke the family of the token with this hash (logout). Unknown hash is a no-op.
pub async fn revoke_family_of(pool: &PgPool, token_hash: &[u8]) -> Result<(), sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let family: Option<Uuid> =
        sqlx::query_scalar("SELECT family_id FROM refresh_tokens WHERE token_hash = $1")
            .bind(token_hash)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(family_id) = family {
        revoke_family(&mut conn, family_id).await?;
    }
    Ok(())
}

async fn revoke_family(conn: &mut PgConnection, family_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = now() WHERE family_id = $1 AND revoked_at IS NULL",
    )
    .bind(family_id)
    .execute(conn)
    .await?;
    Ok(())
}

pub struct AuditEntry<'a> {
    pub workspace_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub action: &'a str,
    pub entity: &'a str,
    pub entity_id: &'a str,
    /// JSON text.
    pub diff: Option<&'a str>,
}

pub async fn insert_audit(db: impl PgExecutor<'_>, e: &AuditEntry<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (id, workspace_id, actor_id, action, entity, entity_id, diff) \
         VALUES ($1, $2, $3, $4, $5, $6, $7::text::jsonb)",
    )
    .bind(Uuid::now_v7())
    .bind(e.workspace_id)
    .bind(e.actor_id)
    .bind(e.action)
    .bind(e.entity)
    .bind(e.entity_id)
    .bind(e.diff)
    .execute(db)
    .await?;
    Ok(())
}
