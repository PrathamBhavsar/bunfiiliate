//! `auth_throttle` rows. The policy (thresholds, lock lengths) lives in `bf_core::throttle`.

use bf_core::throttle::{Policy, State};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

pub const RESET_USER: &str = "reset_user";
pub const LOGIN_IP: &str = "login_ip";

type Row = (i32, DateTime<Utc>, Option<DateTime<Utc>>, i32);

/// Is `(scope, key)` locked at `now`?
pub async fn is_locked(
    db: &PgPool,
    scope: &str,
    key: &str,
    now: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let locked: Option<Option<bool>> = sqlx::query_scalar(
        "SELECT locked_until > $3 FROM auth_throttle WHERE scope = $1 AND key = $2",
    )
    .bind(scope)
    .bind(key)
    .bind(now)
    .fetch_optional(db)
    .await?;
    Ok(locked.flatten().unwrap_or(false))
}

/// Count one failure. The row is locked for the read-modify-write, so concurrent failures all
/// count.
pub async fn record_failure(
    db: &PgPool,
    scope: &str,
    key: &str,
    now: DateTime<Utc>,
    policy: &Policy,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO auth_throttle (scope, key, window_started_at) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(scope)
    .bind(key)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let (failures, window_started_at, locked_until, lock_count): Row = sqlx::query_as(
        "SELECT failures, window_started_at, locked_until, lock_count FROM auth_throttle \
         WHERE scope = $1 AND key = $2 FOR UPDATE",
    )
    .bind(scope)
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;
    let next = State {
        failures,
        window_started_at,
        locked_until,
        lock_count,
    }
    .failed(now, policy);
    sqlx::query(
        "UPDATE auth_throttle SET failures = $3, window_started_at = $4, locked_until = $5, \
         lock_count = $6 WHERE scope = $1 AND key = $2",
    )
    .bind(scope)
    .bind(key)
    .bind(next.failures)
    .bind(next.window_started_at)
    .bind(next.locked_until)
    .bind(next.lock_count)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Forget `(scope, key)` after a success.
pub async fn clear(
    db: impl sqlx::PgExecutor<'_>,
    scope: &str,
    key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM auth_throttle WHERE scope = $1 AND key = $2")
        .bind(scope)
        .bind(key)
        .execute(db)
        .await?;
    Ok(())
}
