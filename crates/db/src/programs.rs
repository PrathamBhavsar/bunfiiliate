//! Programs, sites, conversion types and commission plans. Every query is scoped to a workspace.

use serde::Serialize;
use sqlx::{PgExecutor, PgPool};
use utoipa::ToSchema;
use uuid::Uuid;

const PROGRAM_COLS: &str = "id, workspace_id, name, currency::text AS currency, \
     attribution_model::text AS attribution_model, cookie_days, auto_approve, hold_days, \
     default_plan_id, public_key, webhook_secret, stripe_secret, paddle_secret";

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct Program {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub currency: String,
    pub attribution_model: String,
    pub cookie_days: i32,
    pub auto_approve: bool,
    pub hold_days: i32,
    pub default_plan_id: Option<Uuid>,
    /// Embedded in site snippets; not a secret.
    pub public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stripe_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paddle_secret: Option<String>,
}

impl Program {
    /// Drop the secrets before showing the program to a non-admin.
    pub fn redact(mut self) -> Self {
        self.webhook_secret = None;
        self.stripe_secret = None;
        self.paddle_secret = None;
        self
    }
}

pub struct NewProgram<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: &'a str,
    pub currency: &'a str,
    pub attribution_model: &'a str,
    pub cookie_days: i32,
    pub auto_approve: bool,
    pub hold_days: i32,
    pub public_key: &'a str,
    pub webhook_secret: &'a str,
}

pub async fn insert_program(
    db: impl PgExecutor<'_>,
    p: &NewProgram<'_>,
) -> Result<Program, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO programs (id, workspace_id, name, currency, attribution_model, cookie_days, \
                               auto_approve, hold_days, public_key, webhook_secret) \
         VALUES ($1, $2, $3, $4, $5::text::attribution_model, $6, $7, $8, $9, $10) \
         RETURNING {PROGRAM_COLS}"
    ))
    .bind(p.id)
    .bind(p.workspace_id)
    .bind(p.name)
    .bind(p.currency)
    .bind(p.attribution_model)
    .bind(p.cookie_days)
    .bind(p.auto_approve)
    .bind(p.hold_days)
    .bind(p.public_key)
    .bind(p.webhook_secret)
    .fetch_one(db)
    .await
}

/// `only_for`: restrict to programs this affiliate is enrolled in.
pub async fn list_programs(
    pool: &PgPool,
    workspace_id: Uuid,
    only_for: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Program>, i64), sqlx::Error> {
    const FILTER: &str = "workspace_id = $1 AND ($2::uuid IS NULL OR id IN \
         (SELECT program_id FROM enrollments WHERE affiliate_id = $2))";
    let items = sqlx::query_as(&format!(
        "SELECT {PROGRAM_COLS} FROM programs WHERE {FILTER} ORDER BY id LIMIT $3 OFFSET $4"
    ))
    .bind(workspace_id)
    .bind(only_for)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!("SELECT count(*) FROM programs WHERE {FILTER}"))
        .bind(workspace_id)
        .bind(only_for)
        .fetch_one(pool)
        .await?;
    Ok((items, total))
}

pub async fn get_program(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
    only_for: Option<Uuid>,
) -> Result<Option<Program>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {PROGRAM_COLS} FROM programs WHERE id = $1 AND workspace_id = $2 \
         AND ($3::uuid IS NULL OR id IN (SELECT program_id FROM enrollments WHERE affiliate_id = $3))"
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(only_for)
    .fetch_optional(db)
    .await
}

#[derive(Default)]
pub struct ProgramPatch<'a> {
    pub name: Option<&'a str>,
    pub attribution_model: Option<&'a str>,
    pub cookie_days: Option<i32>,
    pub auto_approve: Option<bool>,
    pub hold_days: Option<i32>,
    pub default_plan_id: Option<Uuid>,
    /// Empty string clears the secret.
    pub stripe_secret: Option<&'a str>,
    pub paddle_secret: Option<&'a str>,
}

pub async fn update_program(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
    p: &ProgramPatch<'_>,
) -> Result<Option<Program>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE programs SET \
            name = COALESCE($3, name), \
            attribution_model = COALESCE($4::text::attribution_model, attribution_model), \
            cookie_days = COALESCE($5, cookie_days), \
            auto_approve = COALESCE($6, auto_approve), \
            hold_days = COALESCE($7, hold_days), \
            default_plan_id = COALESCE($8, default_plan_id), \
            stripe_secret = CASE WHEN $9::text IS NULL THEN stripe_secret ELSE NULLIF($9, '') END, \
            paddle_secret = CASE WHEN $10::text IS NULL THEN paddle_secret ELSE NULLIF($10, '') END \
         WHERE id = $1 AND workspace_id = $2 RETURNING {PROGRAM_COLS}"
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(p.name)
    .bind(p.attribution_model)
    .bind(p.cookie_days)
    .bind(p.auto_approve)
    .bind(p.hold_days)
    .bind(p.default_plan_id)
    .bind(p.stripe_secret)
    .bind(p.paddle_secret)
    .fetch_optional(db)
    .await
}

pub async fn set_webhook_secret(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
    secret: &str,
) -> Result<Option<Program>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE programs SET webhook_secret = $3 WHERE id = $1 AND workspace_id = $2 \
         RETURNING {PROGRAM_COLS}"
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(secret)
    .fetch_optional(db)
    .await
}

/// Cascades to sites, types, plans, enrollments and codes. Fails with a foreign-key
/// violation once the program has clicks or conversions.
pub async fn delete_program(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM programs WHERE id = $1 AND workspace_id = $2")
        .bind(id)
        .bind(workspace_id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn program_exists(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM programs WHERE id = $1 AND workspace_id = $2)")
        .bind(id)
        .bind(workspace_id)
        .fetch_one(db)
        .await
}

// ---- sites

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct Site {
    pub domain: String,
}

pub async fn insert_site(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    program_id: Uuid,
    domain: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO program_sites (program_id, domain) \
         SELECT id, $3 FROM programs WHERE id = $2 AND workspace_id = $1",
    )
    .bind(workspace_id)
    .bind(program_id)
    .bind(domain)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn list_sites(
    pool: &PgPool,
    workspace_id: Uuid,
    program_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Site>, i64), sqlx::Error> {
    const FROM: &str = "FROM program_sites s JOIN programs p ON p.id = s.program_id \
                        WHERE p.workspace_id = $1 AND s.program_id = $2";
    let items = sqlx::query_as(&format!(
        "SELECT s.domain {FROM} ORDER BY s.domain LIMIT $3 OFFSET $4"
    ))
    .bind(workspace_id)
    .bind(program_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!("SELECT count(*) {FROM}"))
        .bind(workspace_id)
        .bind(program_id)
        .fetch_one(pool)
        .await?;
    Ok((items, total))
}

pub async fn delete_site(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    program_id: Uuid,
    domain: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM program_sites s USING programs p \
         WHERE p.id = s.program_id AND p.workspace_id = $1 AND s.program_id = $2 AND s.domain = $3",
    )
    .bind(workspace_id)
    .bind(program_id)
    .bind(domain)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Every site domain of a program (a handful at most).
pub async fn site_domains(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    program_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT s.domain FROM program_sites s JOIN programs p ON p.id = s.program_id \
         WHERE p.workspace_id = $1 AND s.program_id = $2 ORDER BY s.domain",
    )
    .bind(workspace_id)
    .bind(program_id)
    .fetch_all(db)
    .await
}

// ---- conversion types

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct ConversionType {
    pub id: Uuid,
    pub program_id: Uuid,
    pub key: String,
    pub name: String,
}

pub async fn insert_conversion_type(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    t: &ConversionType,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO conversion_types (id, program_id, key, name) \
         SELECT $3, id, $4, $5 FROM programs WHERE id = $2 AND workspace_id = $1",
    )
    .bind(workspace_id)
    .bind(t.program_id)
    .bind(t.id)
    .bind(&t.key)
    .bind(&t.name)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn list_conversion_types(
    pool: &PgPool,
    workspace_id: Uuid,
    program_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(Vec<ConversionType>, i64), sqlx::Error> {
    const FROM: &str = "FROM conversion_types t JOIN programs p ON p.id = t.program_id \
                        WHERE p.workspace_id = $1 AND t.program_id = $2";
    let items = sqlx::query_as(&format!(
        "SELECT t.id, t.program_id, t.key, t.name {FROM} ORDER BY t.id LIMIT $3 OFFSET $4"
    ))
    .bind(workspace_id)
    .bind(program_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!("SELECT count(*) {FROM}"))
        .bind(workspace_id)
        .bind(program_id)
        .fetch_one(pool)
        .await?;
    Ok((items, total))
}

pub async fn delete_conversion_type(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    program_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM conversion_types t USING programs p \
         WHERE p.id = t.program_id AND p.workspace_id = $1 AND t.program_id = $2 AND t.id = $3",
    )
    .bind(workspace_id)
    .bind(program_id)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn conversion_type_in_program(
    db: impl PgExecutor<'_>,
    program_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM conversion_types WHERE id = $2 AND program_id = $1)",
    )
    .bind(program_id)
    .bind(id)
    .fetch_one(db)
    .await
}

// ---- commission plans

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct Plan {
    pub id: Uuid,
    pub program_id: Uuid,
    pub conversion_type_id: Option<Uuid>,
    pub kind: String,
    pub rate_bps: Option<i32>,
    pub amount_cents: Option<i64>,
    pub duration: String,
    pub months: Option<i32>,
}

const PLAN_COLS: &str = "id, program_id, conversion_type_id, kind::text AS kind, rate_bps, \
     amount_cents, duration::text AS duration, months";

pub async fn insert_plan(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    p: &Plan,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO commission_plans (id, program_id, conversion_type_id, kind, rate_bps, \
                                       amount_cents, duration, months) \
         SELECT $3, id, $4, $5::text::commission_kind, $6, $7, $8::text::commission_duration, $9 \
         FROM programs WHERE id = $2 AND workspace_id = $1",
    )
    .bind(workspace_id)
    .bind(p.program_id)
    .bind(p.id)
    .bind(p.conversion_type_id)
    .bind(&p.kind)
    .bind(p.rate_bps)
    .bind(p.amount_cents)
    .bind(&p.duration)
    .bind(p.months)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn list_plans(
    pool: &PgPool,
    workspace_id: Uuid,
    program_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Plan>, i64), sqlx::Error> {
    const FROM: &str = "FROM commission_plans WHERE program_id = $2 \
                        AND program_id IN (SELECT id FROM programs WHERE workspace_id = $1)";
    let items = sqlx::query_as(&format!(
        "SELECT {PLAN_COLS} {FROM} ORDER BY id LIMIT $3 OFFSET $4"
    ))
    .bind(workspace_id)
    .bind(program_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!("SELECT count(*) {FROM}"))
        .bind(workspace_id)
        .bind(program_id)
        .fetch_one(pool)
        .await?;
    Ok((items, total))
}

/// Fails with a foreign-key violation while the plan is a program default or an override.
pub async fn delete_plan(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    program_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM commission_plans WHERE id = $3 AND program_id = $2 \
         AND program_id IN (SELECT id FROM programs WHERE workspace_id = $1)",
    )
    .bind(workspace_id)
    .bind(program_id)
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn plan_in_program(
    db: impl PgExecutor<'_>,
    program_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commission_plans WHERE id = $2 AND program_id = $1)",
    )
    .bind(program_id)
    .bind(id)
    .fetch_one(db)
    .await
}

/// Drop codes of a program whose landing host is no longer one of its sites (or a subdomain).
pub async fn delete_offsite_codes(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    program_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM referral_codes c USING programs p \
         WHERE p.id = c.program_id AND p.workspace_id = $2 AND c.program_id = $1 AND NOT EXISTS ( \
           SELECT 1 FROM program_sites s WHERE s.program_id = c.program_id AND ( \
             lower(substring(c.landing_url from '^https?://([^/?#:]*)')) = s.domain \
             OR lower(substring(c.landing_url from '^https?://([^/?#:]*)')) LIKE '%.' || s.domain))",
    )
    .bind(program_id)
    .bind(workspace_id)
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}
