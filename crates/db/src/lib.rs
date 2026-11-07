//! Postgres access: pool, migrations, repos.

pub mod affiliates;
pub mod auth;
pub mod ledger;
pub mod ops;
pub mod payouts;
pub mod programs;
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

/// Apply embedded migrations.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
