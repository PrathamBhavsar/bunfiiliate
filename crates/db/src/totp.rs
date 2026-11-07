//! TOTP enrolment state and recovery-code rows.

use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Totp {
    pub user_id: Uuid,
    pub secret_enc: String,
    pub confirmed: bool,
    pub last_used_step: Option<i64>,
}

pub async fn get(db: impl PgExecutor<'_>, user_id: Uuid) -> Result<Option<Totp>, sqlx::Error> {
    sqlx::query_as(
        "SELECT user_id, secret_enc, confirmed_at IS NOT NULL AS confirmed, last_used_step \
         FROM user_totp WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

/// Store a pending secret, replacing an unconfirmed one. `false` when already confirmed.
pub async fn upsert_pending(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
    secret_enc: &str,
    sealed: bool,
) -> Result<bool, sqlx::Error> {
    let row: Option<i32> = sqlx::query_scalar(
        "INSERT INTO user_totp (user_id, secret_enc, secret_enc_at) VALUES ($1, $2, CASE WHEN $3 THEN now() END) \
         ON CONFLICT (user_id) DO UPDATE \
           SET secret_enc = $2, secret_enc_at = CASE WHEN $3 THEN now() END, last_used_step = NULL, created_at = now() \
           WHERE user_totp.confirmed_at IS NULL \
         RETURNING 1",
    )
    .bind(user_id)
    .bind(secret_enc)
    .bind(sealed)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Atomically record `step` as used. `false` means a replay (step not newer than the last).
pub async fn claim_step(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
    step: i64,
) -> Result<bool, sqlx::Error> {
    let row: Option<i32> = sqlx::query_scalar(
        "UPDATE user_totp SET last_used_step = $2 \
         WHERE user_id = $1 AND (last_used_step IS NULL OR last_used_step < $2) RETURNING 1",
    )
    .bind(user_id)
    .bind(step)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Mark enrolment confirmed. `false` if it was already confirmed or is missing.
pub async fn confirm(db: impl PgExecutor<'_>, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let row: Option<i32> = sqlx::query_scalar(
        "UPDATE user_totp SET confirmed_at = now() \
         WHERE user_id = $1 AND confirmed_at IS NULL RETURNING 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

pub async fn delete(db: impl PgExecutor<'_>, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM user_totp WHERE user_id = $1")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Replace every recovery code of the user with `hashes` (argon2 PHC strings).
pub async fn replace_recovery_codes(
    tx: &mut PgConnection,
    user_id: Uuid,
    hashes: &[String],
) -> Result<(), sqlx::Error> {
    delete_recovery_codes(&mut *tx, user_id).await?;
    for h in hashes {
        sqlx::query("INSERT INTO user_recovery_codes (id, user_id, code_hash) VALUES ($1, $2, $3)")
            .bind(Uuid::now_v7())
            .bind(user_id)
            .bind(h)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

pub async fn delete_recovery_codes(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn recovery_codes_remaining(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM user_recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(db)
    .await
}

/// Unused recovery codes as `(id, argon2 hash)`, for the reset flow to verify against.
pub async fn unused_recovery_codes(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, code_hash FROM user_recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

/// Atomically consume one code. `false` if it was already used.
pub async fn consume_recovery_code(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
    code_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<Uuid> = sqlx::query_scalar(
        "UPDATE user_recovery_codes SET used_at = now() \
         WHERE id = $2 AND user_id = $1 AND used_at IS NULL RETURNING id",
    )
    .bind(user_id)
    .bind(code_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}
