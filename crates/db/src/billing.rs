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

/// Every plan, cheapest first.
pub async fn list_plans(db: impl PgExecutor<'_>) -> Result<Vec<Plan>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, key, name, price_cents, currency::text AS currency, max_programs, \
                max_affiliates, max_monthly_clicks, is_default \
         FROM billing_plans ORDER BY price_cents, key",
    )
    .fetch_all(db)
    .await
}

/// A plan by its key.
pub async fn plan_by_key(db: impl PgExecutor<'_>, key: &str) -> Result<Option<Plan>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, key, name, price_cents, currency::text AS currency, max_programs, \
                max_affiliates, max_monthly_clicks, is_default \
         FROM billing_plans WHERE key = $1",
    )
    .bind(key)
    .fetch_optional(db)
    .await
}

/// The LemonSqueezy variant id mapped to a plan, if any.
pub async fn lemonsqueezy_variant(
    db: impl PgExecutor<'_>,
    plan_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let v: Option<Option<String>> =
        sqlx::query_scalar("SELECT lemonsqueezy_variant_id FROM billing_plans WHERE id = $1")
            .bind(plan_id)
            .fetch_optional(db)
            .await?;
    Ok(v.flatten())
}

#[derive(Debug, Clone, FromRow)]
pub struct Subscription {
    pub plan_id: Uuid,
    pub provider: String,
    pub provider_subscription_id: Option<String>,
    pub status: String,
    pub current_period_end: Option<DateTime<Utc>>,
}

pub async fn subscription_for_workspace(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
) -> Result<Option<Subscription>, sqlx::Error> {
    sqlx::query_as(
        "SELECT plan_id, provider, provider_subscription_id, status::text AS status, \
                current_period_end \
         FROM subscriptions WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(db)
    .await
}

/// Put the workspace on `plan_id` now (mock checkout), active, no provider ordering.
pub async fn activate_plan(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    plan_id: Uuid,
    provider: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO subscriptions (workspace_id, plan_id, provider, status) \
         VALUES ($1, $2, $3, 'active') \
         ON CONFLICT (workspace_id) DO UPDATE SET plan_id = EXCLUDED.plan_id, \
           provider = EXCLUDED.provider, status = 'active', provider_subscription_id = NULL, \
           updated_at = now()",
    )
    .bind(workspace_id)
    .bind(plan_id)
    .bind(provider)
    .execute(db)
    .await?;
    Ok(())
}

/// Record a provider event id. `false` = already seen (a replay).
pub async fn claim_event(
    db: impl PgExecutor<'_>,
    provider: &str,
    event_id: &str,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query(
        "INSERT INTO billing_events (provider, event_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(provider)
    .bind(event_id)
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}

pub struct EventUpdate<'a> {
    pub workspace_id: Uuid,
    pub provider: &'a str,
    pub provider_subscription_id: Option<&'a str>,
    pub status: &'a str,
    pub plan_key: Option<&'a str>,
    pub event_at: DateTime<Utc>,
    pub current_period_end: Option<DateTime<Utc>>,
}

/// Apply a provider event. `false` when the workspace does not exist or the stored subscription
/// already reflects a newer event (events are ordered by the provider's `updated_at`).
/// A canceled subscription drops back to the default plan.
pub async fn apply_event(
    db: impl PgExecutor<'_>,
    e: &EventUpdate<'_>,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query(
        "INSERT INTO subscriptions (workspace_id, plan_id, provider, provider_subscription_id, \
                                    status, current_period_end, provider_event_at) \
         SELECT w.id, \
                CASE WHEN $4::subscription_status = 'canceled' \
                     THEN (SELECT id FROM billing_plans WHERE is_default) \
                     ELSE COALESCE((SELECT id FROM billing_plans WHERE key = $5), \
                                   (SELECT plan_id FROM subscriptions WHERE workspace_id = w.id), \
                                   (SELECT id FROM billing_plans WHERE is_default)) END, \
                $2, $3, $4::subscription_status, $6, $7 \
         FROM workspaces w WHERE w.id = $1 \
         ON CONFLICT (workspace_id) DO UPDATE SET plan_id = EXCLUDED.plan_id, \
           provider = EXCLUDED.provider, \
           provider_subscription_id = EXCLUDED.provider_subscription_id, \
           status = EXCLUDED.status, current_period_end = EXCLUDED.current_period_end, \
           provider_event_at = EXCLUDED.provider_event_at, updated_at = now() \
         WHERE subscriptions.provider_event_at IS NULL \
            OR subscriptions.provider_event_at <= EXCLUDED.provider_event_at",
    )
    .bind(e.workspace_id)
    .bind(e.provider)
    .bind(e.provider_subscription_id)
    .bind(e.status)
    .bind(e.plan_key)
    .bind(e.current_period_end)
    .bind(e.event_at)
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// The sealed webhook secret and its row id (the AAD binding).
pub async fn provider_secret(
    db: impl PgExecutor<'_>,
    provider: &str,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as("SELECT id, webhook_secret FROM billing_providers WHERE provider = $1")
        .bind(provider)
        .fetch_optional(db)
        .await
}

/// Row id of a configured provider.
pub async fn provider_id(
    db: impl PgExecutor<'_>,
    provider: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM billing_providers WHERE provider = $1")
        .bind(provider)
        .fetch_optional(db)
        .await
}

/// Store a (sealed) webhook secret for `provider`. `id` is the row id the value was sealed to:
/// an existing row's id, or a new one. `false` = lost a race against another first insert.
pub async fn upsert_provider_secret(
    db: impl PgExecutor<'_>,
    id: Uuid,
    provider: &str,
    stored: &str,
    sealed: bool,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query(
        "INSERT INTO billing_providers (id, provider, webhook_secret, secret_enc_at) \
         VALUES ($1, $2, $3, CASE WHEN $4 THEN now() END) \
         ON CONFLICT (provider) DO UPDATE SET webhook_secret = EXCLUDED.webhook_secret, \
           secret_enc_at = EXCLUDED.secret_enc_at, updated_at = now() \
         WHERE billing_providers.id = EXCLUDED.id",
    )
    .bind(id)
    .bind(provider)
    .bind(stored)
    .bind(sealed)
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Providers with a secret configured.
pub async fn configured_providers(db: impl PgExecutor<'_>) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT provider FROM billing_providers ORDER BY 1")
        .fetch_all(db)
        .await
}
