//! Postgres access: pool, migrations, repos.

pub mod affiliates;
pub mod auth;
pub mod billing;
pub mod ledger;
pub mod ops;
pub mod partitions;
pub mod payouts;
pub mod programs;
pub mod reset_tokens;
pub mod throttle;
pub mod totp;
pub mod tracker;
pub mod workspaces;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Open a pool against `url`.
pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().max_connections(10).connect(url).await
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Apply embedded migrations.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

/// Highest migration version embedded in this binary.
pub fn embedded_head() -> i64 {
    MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0)
}

/// Highest successfully applied migration version in the database (0 if none).
pub async fn applied_head(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(max(version), 0) FROM _sqlx_migrations WHERE success")
        .fetch_one(pool)
        .await
}
