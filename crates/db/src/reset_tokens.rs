//! Admin-issued password reset tokens (hash only).

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

/// Store a new token for `user_id`, dropping any older unused ones.
pub async fn replace(
    db: &mut PgConnection,
    user_id: Uuid,
    issued_by: Uuid,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM password_reset_tokens WHERE user_id = $1 AND used_at IS NULL")
        .bind(user_id)
        .execute(&mut *db)
        .await?;
    sqlx::query(
        "INSERT INTO password_reset_tokens (id, user_id, token_hash, issued_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(token_hash)
    .bind(issued_by)
    .bind(expires_at)
    .execute(&mut *db)
    .await
    .map(|_| ())
}

/// Atomically mark a live token used; returns its user. `None` = unknown, used or expired.
pub async fn claim(db: &mut PgConnection, token_hash: &[u8]) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "UPDATE password_reset_tokens SET used_at = now() \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() RETURNING user_id",
    )
    .bind(token_hash)
    .fetch_optional(&mut *db)
    .await
}
