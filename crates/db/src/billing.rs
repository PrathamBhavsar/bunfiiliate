//! Billing plans, subscriptions and usage counts.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct Plan {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub price_cents: i64,
    pub currency: String,
    pub max_programs: Option<i32>,
    pub max_affiliates: Option<i32>,
    pub max_monthly_clicks: Option<i64>,
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub programs: i64,
    pub affiliates: i64,
    pub monthly_clicks: i64,
}

/// Insert a workspace and its default-plan subscription in one statement.
pub async fn create_workspace_with_subscription(
    db: impl PgExecutor<'_>,
    id: Uuid,
    name: &str,
    slug: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "WITH w AS (INSERT INTO workspaces (id, name, slug) VALUES ($1, $2, $3) RETURNING id) \
         INSERT INTO subscriptions (workspace_id, plan_id) \
         SELECT w.id, (SELECT id FROM billing_plans WHERE is_default) FROM w",
    )
    .bind(id)
    .bind(name)
    .bind(slug)
    .execute(db)
    .await?;
    Ok(())
}

/// The workspace's plan; falls back to the default plan if it has no subscription row.
pub async fn plan_for_workspace(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
) -> Result<Plan, sqlx::Error> {
    sqlx::query_as(
        "SELECT p.id, p.key, p.name, p.price_cents, p.currency::text AS currency, \
                p.max_programs, p.max_affiliates, p.max_monthly_clicks, p.is_default \
         FROM billing_plans p \
         WHERE p.id = COALESCE((SELECT plan_id FROM subscriptions WHERE workspace_id = $1), \
                               (SELECT id FROM billing_plans WHERE is_default))",
    )
    .bind(workspace_id)
    .fetch_one(db)
    .await
}

/// Live counts: programs, affiliates, clicks in the trailing 30 days.
pub async fn usage_for_workspace(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
) -> Result<Usage, sqlx::Error> {
    usage_at(db, workspace_id, Utc::now()).await
}

pub async fn usage_at(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Usage, sqlx::Error> {
    let (programs, affiliates, monthly_clicks): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM programs WHERE workspace_id = $1), \
                (SELECT count(*) FROM affiliates WHERE workspace_id = $1), \
                (SELECT count(*) FROM clicks c JOIN programs p ON p.id = c.program_id \
                  WHERE p.workspace_id = $1 AND c.ts > $2 - interval '30 days' AND c.ts <= $2)",
    )
    .bind(workspace_id)
    .bind(now)
    .fetch_one(db)
    .await?;
    Ok(Usage {
        programs,
        affiliates,
        monthly_clicks,
    })
}
