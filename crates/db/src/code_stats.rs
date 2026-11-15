//! Per-referral-code stats (C30). Clicks are counted live from `clicks` with a `ts` bound
//! (month partitions outside the range are pruned); conversions come from `conversions`,
//! attributed to the code of the click behind the customer (`customers.first_click_id`).
//! Reversed and rejected conversions do not count (same rule as the rollups).

use chrono::NaiveDate;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

/// A code resolved inside a workspace.
#[derive(sqlx::FromRow)]
pub struct CodeRef {
    pub program_id: Uuid,
    pub affiliate_id: Uuid,
    pub currency: String,
}

/// `$1` program, `$2` code, `$3` range start (timestamptz), `$4` range end, exclusive.
/// `ts` is bounded on both sides so month partitions outside the range are pruned.
pub const CLICK_SQL: &str = "\
    SELECT k.ts, k.country::text AS country, k.referrer_host, k.fraud_reason \
    FROM clicks k \
    WHERE k.program_id = $1 AND k.code = $2 AND k.ts >= $3 AND k.ts < $4";

/// Resolve `code` through its program into `workspace_id`; `affiliate` = it must be theirs.
/// `None` for an unknown, foreign or other-workspace code (callers answer 404).
pub async fn find_code(
    db: impl PgExecutor<'_>,
    workspace_id: Uuid,
    code: &str,
    affiliate: Option<Uuid>,
) -> Result<Option<CodeRef>, sqlx::Error> {
    sqlx::query_as(
        "SELECT c.program_id, c.affiliate_id, p.currency::text AS currency \
         FROM referral_codes c JOIN programs p ON p.id = c.program_id \
         WHERE p.workspace_id = $1 AND c.code = $2 AND ($3::uuid IS NULL OR c.affiliate_id = $3)",
    )
    .bind(workspace_id)
    .bind(code)
    .bind(affiliate)
    .fetch_optional(db)
    .await
}

#[derive(Default, sqlx::FromRow)]
pub struct Totals {
    pub clicks: i64,
    pub flagged_clicks: i64,
    pub conversions: i64,
    pub revenue_cents: i64,
    pub commission_cents: i64,
}

#[derive(sqlx::FromRow)]
pub struct DayRow {
    pub day: NaiveDate,
    pub clicks: i64,
    pub conversions: i64,
    pub commission_cents: i64,
}

#[derive(sqlx::FromRow)]
pub struct CountryRow {
    pub country: String,
    pub clicks: i64,
}

#[derive(sqlx::FromRow)]
pub struct ReferrerRow {
    pub host: String,
    pub clicks: i64,
}

pub struct CodeStats {
    pub totals: Totals,
    pub series: Vec<DayRow>,
    pub top_countries: Vec<CountryRow>,
    pub top_referrers: Vec<ReferrerRow>,
}

const CONV_SQL: &str = "\
    SELECT cv.occurred_at, cv.amount_cents, cv.commission_cents \
    FROM conversions cv \
    JOIN customers cu ON cu.id = cv.customer_id \
    JOIN clicks k ON k.id = cu.first_click_id \
    WHERE cv.program_id = $1 AND cv.affiliate_id = $5 AND k.code = $2 \
      AND cv.status IN ('pending', 'approved', 'paid') \
      AND cv.occurred_at >= $3 AND cv.occurred_at < $4";

/// Stats of one code for the days `from..=to` (UTC), zero-filled per day.
pub async fn code_stats(
    pool: &PgPool,
    code: &str,
    r: &CodeRef,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<CodeStats, sqlx::Error> {
    // Bounds as timestamptz at UTC midnight; `to` is inclusive so the end is the next day.
    let bounds = "SELECT ($3::date::timestamp AT TIME ZONE 'UTC') AS lo, \
                         (($4::date + 1)::timestamp AT TIME ZONE 'UTC') AS hi";
    let (lo, hi): (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as(bounds)
            .bind(r.program_id)
            .bind(code)
            .bind(from)
            .bind(to)
            .fetch_one(pool)
            .await?;

    let clicks_cte = format!("clicks_in AS ({CLICK_SQL})");
    let conv_cte = format!("conv_in AS ({CONV_SQL})");

    let totals: Totals = sqlx::query_as(&format!(
        "WITH {clicks_cte}, {conv_cte} SELECT \
           (SELECT count(*) FROM clicks_in)::bigint AS clicks, \
           (SELECT count(*) FROM clicks_in WHERE fraud_reason IS NOT NULL)::bigint AS flagged_clicks, \
           (SELECT count(*) FROM conv_in)::bigint AS conversions, \
           (SELECT coalesce(sum(amount_cents), 0) FROM conv_in)::bigint AS revenue_cents, \
           (SELECT coalesce(sum(commission_cents), 0) FROM conv_in)::bigint AS commission_cents"
    ))
    .bind(r.program_id)
    .bind(code)
    .bind(lo)
    .bind(hi)
    .bind(r.affiliate_id)
    .fetch_one(pool)
    .await?;

    let series: Vec<DayRow> = sqlx::query_as(&format!(
        "WITH {clicks_cte}, {conv_cte}, \
         days AS (SELECT d::date AS day FROM generate_series($6::date, $7::date, interval '1 day') d), \
         c AS (SELECT (ts AT TIME ZONE 'UTC')::date AS day, count(*) AS n FROM clicks_in GROUP BY 1), \
         v AS (SELECT (occurred_at AT TIME ZONE 'UTC')::date AS day, count(*) AS n, \
                      sum(commission_cents) AS commission FROM conv_in GROUP BY 1) \
         SELECT days.day, coalesce(c.n, 0)::bigint AS clicks, coalesce(v.n, 0)::bigint AS conversions, \
                coalesce(v.commission, 0)::bigint AS commission_cents \
         FROM days LEFT JOIN c USING (day) LEFT JOIN v USING (day) ORDER BY days.day"
    ))
    .bind(r.program_id)
    .bind(code)
    .bind(lo)
    .bind(hi)
    .bind(r.affiliate_id)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;

    let top_countries: Vec<CountryRow> = sqlx::query_as(&format!(
        "WITH {clicks_cte} SELECT country, count(*)::bigint AS clicks FROM clicks_in \
         WHERE country IS NOT NULL GROUP BY country ORDER BY clicks DESC, country LIMIT 10"
    ))
    .bind(r.program_id)
    .bind(code)
    .bind(lo)
    .bind(hi)
    .fetch_all(pool)
    .await?;

    let top_referrers: Vec<ReferrerRow> = sqlx::query_as(&format!(
        "WITH {clicks_cte} SELECT referrer_host AS host, count(*)::bigint AS clicks FROM clicks_in \
         WHERE referrer_host IS NOT NULL AND referrer_host <> '' \
         GROUP BY referrer_host ORDER BY clicks DESC, referrer_host LIMIT 10"
    ))
    .bind(r.program_id)
    .bind(code)
    .bind(lo)
    .bind(hi)
    .fetch_all(pool)
    .await?;

    Ok(CodeStats {
        totals,
        series,
        top_countries,
        top_referrers,
    })
}
