//! Affiliates, enrollments and referral codes. Every query is scoped to a workspace.

use serde::Serialize;
use sqlx::{PgExecutor, PgPool};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct Affiliate {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    /// `pending`, `active` or `suspended`.
    pub status: String,
    pub payout_email: Option<String>,
    pub payout_min_cents: i64,
}

const AFFILIATE_SELECT: &str = "SELECT a.id, a.workspace_id, a.user_id, u.email::text AS email, \
     a.status::text AS status, a.payout_email::text AS payout_email, a.payout_min_cents \
     FROM affiliates a JOIN users u ON u.id = a.user_id";

pub async fn insert_affiliate(
    db: impl PgExecutor<'_>,
    id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    payout_email: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO affiliates (id, workspace_id, user_id, payout_email) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(user_id)
    .bind(payout_email)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn list_affiliates(
    pool: &PgPool,
    workspace_id: Uuid,
    status: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Affiliate>, i64), sqlx::Error> {
    const FILTER: &str = "a.workspace_id = $1 AND ($2::text IS NULL OR a.status::text = $2)";
    let items = sqlx::query_as(&format!(
        "{AFFILIATE_SELECT} WHERE {FILTER} ORDER BY a.id LIMIT $3 OFFSET $4"
    ))
    .bind(workspace_id)
    .bind(status)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!("SELECT count(*) FROM affiliates a WHERE {FILTER}"))
        .bind(workspace_id)
        .bind(status)
        .fetch_one(pool)
        .await?;
    Ok((items, total))
}

pub async fn get_affiliate(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<Option<Affiliate>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{AFFILIATE_SELECT} WHERE a.id = $1 AND a.workspace_id = $2"
    ))
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(db)
    .await
}

pub async fn affiliate_for_user(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<Affiliate>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{AFFILIATE_SELECT} WHERE a.user_id = $1 AND a.workspace_id = $2"
    ))
    .bind(user_id)
    .bind(workspace_id)
    .fetch_optional(db)
    .await
}

/// Move to `to` if the current status is one of `from`. `false`: not found or wrong status.
pub async fn set_affiliate_status(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
    from: &[&str],
    to: &str,
) -> Result<bool, sqlx::Error> {
    let from: Vec<String> = from.iter().map(|s| (*s).to_string()).collect();
    let res = sqlx::query(
        "UPDATE affiliates SET status = $4::text::affiliate_status \
         WHERE id = $1 AND workspace_id = $2 AND status::text = ANY($3)",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(from)
    .bind(to)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn update_payout(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    id: Uuid,
    payout_email: Option<Option<&str>>,
    payout_min_cents: Option<i64>,
) -> Result<bool, sqlx::Error> {
    // `payout_email`: None leaves it, Some(None) clears it, Some(Some(e)) sets it.
    let res = sqlx::query(
        "UPDATE affiliates SET payout_email = CASE WHEN $5 THEN $3 ELSE payout_email END, \
                payout_min_cents = COALESCE($4, payout_min_cents) \
         WHERE id = $1 AND workspace_id = $2",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(payout_email.flatten())
    .bind(payout_min_cents)
    .bind(payout_email.is_some())
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

// ---- enrollments

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct Enrollment {
    pub affiliate_id: Uuid,
    pub program_id: Uuid,
    pub plan_override_id: Option<Uuid>,
}

/// Enroll, or change the plan override of an existing enrollment. `None`: the program,
/// affiliate or plan is not in this workspace / program.
pub async fn upsert_enrollment(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    program_id: Uuid,
    affiliate_id: Uuid,
    plan_override_id: Option<Uuid>,
) -> Result<Option<Enrollment>, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO enrollments (affiliate_id, program_id, plan_override_id) \
         SELECT a.id, p.id, $4 FROM affiliates a, programs p \
         WHERE a.id = $1 AND p.id = $2 AND a.workspace_id = $3 AND p.workspace_id = $3 \
           AND ($4::uuid IS NULL OR EXISTS \
                (SELECT 1 FROM commission_plans c WHERE c.id = $4 AND c.program_id = p.id)) \
         ON CONFLICT (affiliate_id, program_id) \
         DO UPDATE SET plan_override_id = EXCLUDED.plan_override_id \
         RETURNING affiliate_id, program_id, plan_override_id",
    )
    .bind(affiliate_id)
    .bind(program_id)
    .bind(workspace_id)
    .bind(plan_override_id)
    .fetch_optional(db)
    .await
}

pub async fn list_enrollments(
    pool: &PgPool,
    workspace_id: Uuid,
    program_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Enrollment>, i64), sqlx::Error> {
    const FROM: &str = "FROM enrollments e JOIN programs p ON p.id = e.program_id \
                        WHERE p.workspace_id = $1 AND e.program_id = $2";
    let items = sqlx::query_as(&format!(
        "SELECT e.affiliate_id, e.program_id, e.plan_override_id {FROM} \
         ORDER BY e.affiliate_id LIMIT $3 OFFSET $4"
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

/// Remove the enrollment and the affiliate's codes for that program, so the tracker
/// stops attributing to them.
pub async fn delete_enrollment(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    program_id: Uuid,
    affiliate_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM enrollments e USING programs p \
         WHERE p.id = e.program_id AND p.workspace_id = $1 AND e.program_id = $2 \
           AND e.affiliate_id = $3",
    )
    .bind(workspace_id)
    .bind(program_id)
    .bind(affiliate_id)
    .execute(&mut *conn)
    .await?;
    if res.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query(
        "DELETE FROM referral_codes c USING programs p \
         WHERE p.id = c.program_id AND p.workspace_id = $3 \
           AND c.program_id = $1 AND c.affiliate_id = $2",
    )
    .bind(program_id)
    .bind(affiliate_id)
    .bind(workspace_id)
    .execute(conn)
    .await?;
    Ok(true)
}

// ---- referral codes

#[derive(Debug, Clone, sqlx::FromRow, Serialize, ToSchema)]
pub struct ReferralCode {
    pub code: String,
    pub affiliate_id: Uuid,
    pub program_id: Uuid,
    pub landing_url: String,
}

/// Create a code for an active, enrolled affiliate. `false`: they are not (or the program
/// is not in this workspace). A taken code is a unique violation.
pub async fn insert_code(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    c: &ReferralCode,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) \
         SELECT $4, a.id, p.id, $5 FROM enrollments e \
         JOIN affiliates a ON a.id = e.affiliate_id JOIN programs p ON p.id = e.program_id \
         WHERE e.affiliate_id = $2 AND e.program_id = $3 \
           AND a.workspace_id = $1 AND p.workspace_id = $1 AND a.status = 'active'",
    )
    .bind(workspace_id)
    .bind(c.affiliate_id)
    .bind(c.program_id)
    .bind(&c.code)
    .bind(&c.landing_url)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// How many codes this affiliate holds, across programs.
pub async fn count_codes(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    affiliate_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM referral_codes c JOIN affiliates a ON a.id = c.affiliate_id \
         WHERE c.affiliate_id = $1 AND a.workspace_id = $2",
    )
    .bind(affiliate_id)
    .bind(workspace_id)
    .fetch_one(db)
    .await
}

pub async fn list_codes(
    pool: &PgPool,
    workspace_id: Uuid,
    affiliate_id: Option<Uuid>,
    program_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<ReferralCode>, i64), sqlx::Error> {
    const FROM: &str = "FROM referral_codes c JOIN programs p ON p.id = c.program_id \
         WHERE p.workspace_id = $1 AND ($2::uuid IS NULL OR c.affiliate_id = $2) \
           AND ($3::uuid IS NULL OR c.program_id = $3)";
    let items = sqlx::query_as(&format!(
        "SELECT c.code, c.affiliate_id, c.program_id, COALESCE(c.landing_url, '') AS landing_url \
         {FROM} ORDER BY c.code LIMIT $4 OFFSET $5"
    ))
    .bind(workspace_id)
    .bind(affiliate_id)
    .bind(program_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(&format!("SELECT count(*) {FROM}"))
        .bind(workspace_id)
        .bind(affiliate_id)
        .bind(program_id)
        .fetch_one(pool)
        .await?;
    Ok((items, total))
}

/// Deactivate = delete: the tracker snapshot is built from the rows that exist.
/// `affiliate_id`: only if the code belongs to this affiliate.
pub async fn delete_code(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    code: &str,
    affiliate_id: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM referral_codes c USING programs p \
         WHERE p.id = c.program_id AND p.workspace_id = $1 AND c.code = $2 \
           AND ($3::uuid IS NULL OR c.affiliate_id = $3)",
    )
    .bind(workspace_id)
    .bind(code)
    .bind(affiliate_id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Drop every code of an affiliate (used on suspend, so the tracker stops attributing).
pub async fn delete_codes_of_affiliate(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    affiliate_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM referral_codes c USING programs p \
         WHERE p.id = c.program_id AND p.workspace_id = $1 AND c.affiliate_id = $2",
    )
    .bind(workspace_id)
    .bind(affiliate_id)
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}
