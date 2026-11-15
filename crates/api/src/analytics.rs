//! Daily rollups (worker tick) and the read side: `/analytics/*` and `/reports/analytics.csv`.
//!
//! The tick rebuilds `daily_rollups` per program from the last rolled-up day minus one
//! (the watermark), so a late click or conversion on yesterday still lands. Clicks and
//! conversions are folded into every dim; a conversion takes its dims from the click
//! behind its customer, `''` when there is none, so each dim sums to `total`. Rejected and
//! reversed conversions do not count. A status change older than the watermark window is
//! not picked up. Days are UTC.

use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::auth::{Admin, Affiliate as AffiliateRole, Any, Auth, Principal};
use crate::error::{AppError, ErrorBody};
use crate::programs::{require_program, viewer};

/// Arbitrary key shared by every worker so only one runs the rollup tick at a time.
const ROLLUP_LOCK: i64 = 0x6266_726c;
/// Days before the last rolled-up day that the next tick rebuilds.
const LOOKBACK_DAYS: i64 = 1;
const MAX_RANGE_DAYS: i64 = 366;
const BREAKDOWN_LIMIT: i64 = 200;

// ---- rollup tick

/// Rebuild the recent window of every program; returns rollup rows written.
/// Returns 0 without work when another worker holds the lock.
pub async fn rollup(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(ROLLUP_LOCK)
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Ok(0);
    }
    // `changed`: the oldest day of any conversion created, approved or reversed since this
    // program's last rebuild, so a late chargeback or a back-dated sale is not left behind.
    let programs: Vec<(Uuid, Option<NaiveDate>, Option<NaiveDate>)> = sqlx::query_as(
        "SELECT p.id, (SELECT max(r.day) FROM daily_rollups r WHERE r.program_id = p.id), \
                (SELECT min((c.occurred_at AT TIME ZONE 'UTC')::date) FROM conversions c \
                 WHERE c.program_id = p.id AND GREATEST(c.created_at, c.approved_at, c.reversed_at) \
                       > COALESCE((SELECT s.ran_at FROM rollup_state s WHERE s.program_id = p.id), \
                                   '-infinity') - interval '5 minutes') \
         FROM programs p",
    )
    .fetch_all(&mut *tx)
    .await?;
    let floor = bf_db::partitions::retention_floor_tx(&mut tx).await?;
    let mut written = 0;
    for (program, last, changed) in programs {
        let from = last.map(|d| d - Duration::days(LOOKBACK_DAYS));
        // A program with no rollups yet rebuilds all history (`None`).
        let from = match (from, changed) {
            (Some(f), Some(c)) => Some(f.min(c)),
            (f, _) => f,
        };
        // Retention dropped the clicks before `floor`: rebuilding those days would erase
        // their rollups and count zero clicks. Never recompute them.
        let from = match (from, floor) {
            (Some(f), Some(fl)) => Some(f.max(fl)),
            (None, Some(fl)) => Some(fl),
            (f, None) => f,
        };
        sqlx::query(
            "DELETE FROM daily_rollups WHERE program_id = $1 AND ($2::date IS NULL OR day >= $2)",
        )
        .bind(program)
        .bind(from)
        .execute(&mut *tx)
        .await?;
        written += sqlx::query(ROLLUP_SQL)
            .bind(program)
            .bind(from)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        sqlx::query(
            "INSERT INTO rollup_state (program_id, ran_at) VALUES ($1, now()) \
             ON CONFLICT (program_id) DO UPDATE SET ran_at = EXCLUDED.ran_at",
        )
        .bind(program)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(written)
}

/// Click leg of the rollup facts. `ts` has a stable lower bound, so month partitions before
/// `$2` are pruned (guarded by `clicks_queries_prune_partitions`). `$2` null = all history.
macro_rules! click_leg {
    () => {
        "SELECT k.affiliate_id, (k.ts AT TIME ZONE 'UTC')::date AS day, k.country::text AS country, \
           k.os, k.browser, k.device, k.referrer_host AS referrer, \
           1::bigint AS clicks, 0::bigint AS conversions, 0::bigint AS revenue, 0::bigint AS commission \
    FROM clicks k \
    WHERE k.program_id = $1 \
      AND k.ts >= COALESCE($2::date::timestamp AT TIME ZONE 'UTC', '-infinity'::timestamptz) "
    };
}

/// The click leg alone, for the partition-pruning test.
pub const ROLLUP_CLICK_LEG: &str = click_leg!();

/// `$1` program, `$2` first day to rebuild (null = all history).
const ROLLUP_SQL: &str = concat!(
    "WITH facts AS ( ",
    click_leg!(),
    "UNION ALL \
    SELECT cv.affiliate_id, (cv.occurred_at AT TIME ZONE 'UTC')::date, k.country::text, \
           k.os, k.browser, k.device, k.referrer_host, \
           0, 1, cv.amount_cents, cv.commission_cents \
    FROM conversions cv \
    JOIN customers cu ON cu.id = cv.customer_id \
    LEFT JOIN clicks k ON k.id = cu.first_click_id \
    WHERE cv.program_id = $1 AND cv.status IN ('pending', 'approved', 'paid') \
      AND ($2::date IS NULL OR cv.occurred_at >= $2::date::timestamp AT TIME ZONE 'UTC') \
) \
",
    "INSERT INTO daily_rollups \
    (program_id, affiliate_id, day, dim, dim_value, clicks, conversions, revenue_cents, commission_cents) \
SELECT $1, f.affiliate_id, f.day, d.dim::rollup_dim, coalesce(d.value, ''), \
       sum(f.clicks)::bigint, sum(f.conversions)::bigint, sum(f.revenue)::bigint, sum(f.commission)::bigint \
FROM facts f \
CROSS JOIN LATERAL (VALUES ('total', NULL::text), ('country', f.country), ('os', f.os), \
    ('browser', f.browser), ('device', f.device), ('referrer', f.referrer)) AS d(dim, value) \
GROUP BY f.affiliate_id, f.day, d.dim, coalesce(d.value, '') \
ON CONFLICT (program_id, affiliate_id, day, dim, dim_value) DO UPDATE SET \
    clicks = EXCLUDED.clicks, conversions = EXCLUDED.conversions, \
    revenue_cents = EXCLUDED.revenue_cents, commission_cents = EXCLUDED.commission_cents"
);

// ---- routes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/analytics/summary", get(summary))
        .route("/api/v1/analytics/summary-all", get(summary_all))
        .route("/api/v1/analytics/timeseries", get(timeseries))
        .route("/api/v1/reports/analytics.csv", get(report_csv))
        .route("/api/v1/referral-codes/{code}/stats", get(code_stats))
        .route("/api/v1/me/referral-codes/{code}/stats", get(me_code_stats))
}

#[derive(Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dim {
    Total,
    Country,
    Os,
    Browser,
    Device,
    Referrer,
}

impl Dim {
    fn as_str(self) -> &'static str {
        match self {
            Self::Total => "total",
            Self::Country => "country",
            Self::Os => "os",
            Self::Browser => "browser",
            Self::Device => "device",
            Self::Referrer => "referrer",
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RangeQuery {
    /// Only this program (must be in the workspace). Default: all programs.
    program_id: Option<Uuid>,
    /// Admins only: one affiliate. Affiliates always see their own data.
    affiliate_id: Option<Uuid>,
    /// First day, `YYYY-MM-DD` UTC, inclusive. Default: 29 days before `to`.
    from: Option<String>,
    /// Last day, `YYYY-MM-DD` UTC, inclusive. Default: today.
    to: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DimQuery {
    program_id: Option<Uuid>,
    affiliate_id: Option<Uuid>,
    from: Option<String>,
    to: Option<String>,
    /// Break the range down by this dimension. Summary: none = no breakdown. CSV: default `total`.
    dim: Option<Dim>,
}

/// The resolved read scope; always inside one workspace.
struct Scope {
    workspace: Uuid,
    program: Option<Uuid>,
    affiliate: Option<Uuid>,
    from: NaiveDate,
    to: NaiveDate,
}

pub(crate) fn parse_day(s: &str) -> Result<NaiveDate, AppError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| AppError::BadRequest("dates are YYYY-MM-DD".into()))
}

async fn scope(
    s: &AppState,
    p: &Principal,
    program: Option<Uuid>,
    affiliate: Option<Uuid>,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Scope, AppError> {
    let (workspace, own) = viewer(s, p).await?;
    if let Some(own) = own
        && affiliate.is_some_and(|a| a != own)
    {
        return Err(AppError::NotFound("affiliate not found"));
    }
    if let Some(id) = program {
        require_program(&mut *s.pool.acquire().await?, workspace, id).await?;
    }
    let to = to.map_or_else(|| Ok(Utc::now().date_naive()), parse_day)?;
    let from = from.map_or_else(|| Ok(to - Duration::days(29)), parse_day)?;
    if from > to || (to - from).num_days() >= MAX_RANGE_DAYS {
        return Err(AppError::BadRequest(format!(
            "from must not be after to, and the range is at most {MAX_RANGE_DAYS} days"
        )));
    }
    Ok(Scope {
        workspace,
        program,
        affiliate: own.or(affiliate),
        from,
        to,
    })
}

/// Rollup rows of a scope as `r`. `$1` workspace, `$2` from, `$3` to, `$4` program, `$5` affiliate.
const SCOPED: &str = "\
    daily_rollups r JOIN programs p ON p.id = r.program_id \
    WHERE p.workspace_id = $1 AND r.day BETWEEN $2 AND $3 \
      AND ($4::uuid IS NULL OR r.program_id = $4) AND ($5::uuid IS NULL OR r.affiliate_id = $5)";

/// Sums over `r` rows; the rate is conversions per 10000 clicks, truncated.
const METRICS: &str = "\
    coalesce(sum(r.clicks), 0)::bigint AS clicks, \
    coalesce(sum(r.conversions), 0)::bigint AS conversions, \
    coalesce(sum(r.revenue_cents), 0)::bigint AS revenue_cents, \
    coalesce(sum(r.commission_cents), 0)::bigint AS commission_cents, \
    CASE WHEN coalesce(sum(r.clicks), 0) = 0 THEN 0 \
         ELSE div(sum(r.conversions) * 10000, sum(r.clicks))::bigint END AS conversion_rate_bps";

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Metrics {
    clicks: i64,
    conversions: i64,
    /// Sum of conversion amounts in the program currency's minor unit.
    revenue_cents: i64,
    commission_cents: i64,
    /// Conversions per 10000 clicks (can exceed 10000: recurring conversions have no click).
    conversion_rate_bps: i64,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Slice {
    /// Value of the dimension; empty = unknown.
    dim_value: String,
    #[sqlx(flatten)]
    #[serde(flatten)]
    metrics: Metrics,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Point {
    /// `YYYY-MM-DD` (UTC).
    day: String,
    #[sqlx(flatten)]
    #[serde(flatten)]
    metrics: Metrics,
}

#[derive(Serialize, ToSchema)]
pub struct Summary {
    from: String,
    to: String,
    #[serde(flatten)]
    totals: Metrics,
    dim: Option<Dim>,
    /// Top 200 values of `dim` by clicks; their sums equal the totals when there are fewer.
    breakdown: Vec<Slice>,
}

/// Totals for a day range, optionally broken down by a dimension.
#[utoipa::path(
    get,
    path = "/api/v1/analytics/summary",
    tag = "analytics",
    security(("cookie" = [])),
    params(DimQuery),
    responses(
        (status = 200, body = Summary),
        (status = 400, body = ErrorBody),
        (status = 404, description = "Unknown program or other affiliate", body = ErrorBody),
    )
)]
pub async fn summary(
    State(s): State<AppState>,
    auth: Auth<Any>,
    Query(q): Query<DimQuery>,
) -> Result<Json<Summary>, AppError> {
    let sc = scope(
        &s,
        &auth.principal,
        q.program_id,
        q.affiliate_id,
        q.from.as_deref(),
        q.to.as_deref(),
    )
    .await?;
    let totals = sqlx::query_as(&format!(
        "SELECT {METRICS} FROM {SCOPED} AND r.dim = 'total'"
    ))
    .bind(sc.workspace)
    .bind(sc.from)
    .bind(sc.to)
    .bind(sc.program)
    .bind(sc.affiliate)
    .fetch_one(&s.pool)
    .await?;
    let breakdown = match q.dim {
        None => Vec::new(),
        Some(dim) => {
            sqlx::query_as(&format!(
                "SELECT r.dim_value, {METRICS} FROM {SCOPED} AND r.dim = $6::rollup_dim \
                 GROUP BY r.dim_value ORDER BY sum(r.clicks) DESC, r.dim_value LIMIT $7"
            ))
            .bind(sc.workspace)
            .bind(sc.from)
            .bind(sc.to)
            .bind(sc.program)
            .bind(sc.affiliate)
            .bind(dim.as_str())
            .bind(BREAKDOWN_LIMIT)
            .fetch_all(&s.pool)
            .await?
        }
    };
    Ok(Json(Summary {
        from: sc.from.to_string(),
        to: sc.to.to_string(),
        totals,
        dim: q.dim,
        breakdown,
    }))
}

/// Totals per day, every day of the range (zeros when idle).
#[utoipa::path(
    get,
    path = "/api/v1/analytics/timeseries",
    tag = "analytics",
    security(("cookie" = [])),
    params(RangeQuery),
    responses(
        (status = 200, body = Vec<Point>),
        (status = 400, body = ErrorBody),
        (status = 404, description = "Unknown program or other affiliate", body = ErrorBody),
    )
)]
pub async fn timeseries(
    State(s): State<AppState>,
    auth: Auth<Any>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<Vec<Point>>, AppError> {
    let sc = scope(
        &s,
        &auth.principal,
        q.program_id,
        q.affiliate_id,
        q.from.as_deref(),
        q.to.as_deref(),
    )
    .await?;
    let points = sqlx::query_as(&format!(
        "SELECT to_char(g.day, 'YYYY-MM-DD') AS day, {METRICS} \
         FROM generate_series($2::date::timestamp, $3::date::timestamp, interval '1 day') AS g(day) \
         LEFT JOIN (SELECT r.* FROM {SCOPED} AND r.dim = 'total') r ON r.day = g.day::date \
         GROUP BY g.day ORDER BY g.day"
    ))
    .bind(sc.workspace)
    .bind(sc.from)
    .bind(sc.to)
    .bind(sc.program)
    .bind(sc.affiliate)
    .fetch_all(&s.pool)
    .await?;
    Ok(Json(points))
}

#[derive(sqlx::FromRow)]
struct CsvRow {
    day: String,
    dim_value: String,
    #[sqlx(flatten)]
    metrics: Metrics,
}

/// One cell: quoted when needed, and a leading `= + - @` is defused (spreadsheet formulas).
fn csv_cell(v: &str) -> String {
    let v = if v.starts_with(['=', '+', '-', '@']) {
        format!("'{v}")
    } else {
        v.to_string()
    };
    if v.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

/// CSV of rollup rows per day and value of `dim` (default `total`).
#[utoipa::path(
    get,
    path = "/api/v1/reports/analytics.csv",
    tag = "reports",
    security(("cookie" = [])),
    params(DimQuery),
    responses(
        (status = 200, description = "text/csv", content_type = "text/csv", body = String),
        (status = 400, body = ErrorBody),
        (status = 404, description = "Unknown program or other affiliate", body = ErrorBody),
    )
)]
pub async fn report_csv(
    State(s): State<AppState>,
    auth: Auth<Any>,
    Query(q): Query<DimQuery>,
) -> Result<([(axum::http::HeaderName, &'static str); 2], String), AppError> {
    let sc = scope(
        &s,
        &auth.principal,
        q.program_id,
        q.affiliate_id,
        q.from.as_deref(),
        q.to.as_deref(),
    )
    .await?;
    let dim = q.dim.unwrap_or(Dim::Total);
    let rows: Vec<CsvRow> = sqlx::query_as(&format!(
        "SELECT to_char(r.day, 'YYYY-MM-DD') AS day, r.dim_value, {METRICS} FROM {SCOPED} \
         AND r.dim = $6::rollup_dim GROUP BY r.day, r.dim_value ORDER BY r.day, r.dim_value"
    ))
    .bind(sc.workspace)
    .bind(sc.from)
    .bind(sc.to)
    .bind(sc.program)
    .bind(sc.affiliate)
    .bind(dim.as_str())
    .fetch_all(&s.pool)
    .await?;
    let mut out = String::from(
        "day,dim,dim_value,clicks,conversions,revenue_cents,commission_cents,conversion_rate_bps\n",
    );
    for r in rows {
        let m = r.metrics;
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            r.day,
            dim.as_str(),
            csv_cell(&r.dim_value),
            m.clicks,
            m.conversions,
            m.revenue_cents,
            m.commission_cents,
            m.conversion_rate_bps
        ));
    }
    Ok((
        [
            (CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                CONTENT_DISPOSITION,
                "attachment; filename=\"analytics.csv\"",
            ),
        ],
        out,
    ))
}

// ---- all programs in one display currency (B06)

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SummaryAllQuery {
    /// ISO 4217 code every program's money is converted to. Default `USD`.
    display_currency: Option<String>,
    /// First day, `YYYY-MM-DD` UTC, inclusive. Default: 29 days before `to`.
    from: Option<String>,
    /// Last day, `YYYY-MM-DD` UTC, inclusive. Default: today.
    to: Option<String>,
}

/// Money of a program that could not be converted: no cached rate on or before the day.
#[derive(Serialize, ToSchema, PartialEq, Debug)]
pub struct Unconverted {
    program_id: Uuid,
    /// The program's own currency; amounts are in its minor unit.
    currency: String,
    revenue_cents: i64,
    commission_cents: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SummaryAll {
    display_currency: String,
    from: String,
    to: String,
    /// Converted revenue of every program with a rate, in `display_currency` cents.
    revenue_cents: i64,
    commission_cents: i64,
    /// Programs left out of the totals; never silently converted 1:1.
    unconverted: Vec<Unconverted>,
}

/// Cached USD-based rates of one currency, oldest first.
type RateHistory = Vec<(NaiveDate, bf_core::Rate)>;

/// Units of `cur` per USD on `day`: the latest cached rate on or before it, never a later one.
fn per_usd(
    rates: &std::collections::HashMap<String, RateHistory>,
    cur: &str,
    day: NaiveDate,
) -> Option<bf_core::Rate> {
    if cur == "USD" {
        return Some(bf_core::Rate::ONE);
    }
    let h = rates.get(cur)?;
    let n = h.partition_point(|(d, _)| *d <= day);
    n.checked_sub(1).map(|i| h[i].1)
}

/// Revenue and commission of every program, converted to one currency day by day with the
/// latest rate on or before each day (cross rates go through USD, rounded half-even to cents).
#[utoipa::path(
    get,
    path = "/api/v1/analytics/summary-all",
    tag = "analytics",
    security(("cookie" = [])),
    params(SummaryAllQuery),
    responses(
        (status = 200, body = SummaryAll),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
    )
)]
pub async fn summary_all(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(q): Query<SummaryAllQuery>,
) -> Result<Json<SummaryAll>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let display = bf_core::Currency::new(
        &q.display_currency
            .as_deref()
            .unwrap_or("USD")
            .to_ascii_uppercase(),
    )
    .ok_or_else(|| AppError::BadRequest("display_currency is a 3-letter code".into()))?;
    let to =
        q.to.as_deref()
            .map_or_else(|| Ok(Utc::now().date_naive()), parse_day)?;
    let from = q
        .from
        .as_deref()
        .map_or_else(|| Ok(to - Duration::days(29)), parse_day)?;
    if from > to || (to - from).num_days() >= MAX_RANGE_DAYS {
        return Err(AppError::BadRequest(format!(
            "from must not be after to, and the range is at most {MAX_RANGE_DAYS} days"
        )));
    }
    let rows: Vec<(Uuid, String, NaiveDate, i64, i64)> = sqlx::query_as(
        "SELECT p.id, p.currency::text, r.day, sum(r.revenue_cents)::bigint, \
                sum(r.commission_cents)::bigint \
         FROM daily_rollups r JOIN programs p ON p.id = r.program_id \
         WHERE p.workspace_id = $1 AND r.dim = 'total' AND r.day BETWEEN $2 AND $3 \
         GROUP BY p.id, p.currency, r.day ORDER BY p.id, r.day",
    )
    .bind(ws)
    .bind(from)
    .bind(to)
    .fetch_all(&s.pool)
    .await?;
    let mut wanted: Vec<String> = rows.iter().map(|r| r.1.clone()).collect();
    wanted.push(display.as_str().to_owned());
    wanted.retain(|c| c != "USD");
    wanted.sort_unstable();
    wanted.dedup();
    // exchange_rates is global (no tenant column); rows older than the range are the fallback
    let cached: Vec<(String, NaiveDate, String)> = sqlx::query_as(
        "SELECT quote::text, as_of, rate::text FROM exchange_rates \
         WHERE base = 'USD' AND quote = ANY($1) AND as_of <= $2 ORDER BY quote, as_of",
    )
    .bind(&wanted)
    .bind(to)
    .fetch_all(&s.pool)
    .await?;
    let mut rates: std::collections::HashMap<String, RateHistory> = Default::default();
    for (cur, as_of, rate) in cached {
        let rate = bf_core::Rate::parse(&rate).map_err(|_| AppError::Internal)?;
        rates.entry(cur).or_default().push((as_of, rate));
    }
    let (mut revenue, mut commission) = (0_i128, 0_i128);
    let mut left: Vec<Unconverted> = Vec::new();
    for (program, cur, day, rev, com) in rows {
        let converted = per_usd(&rates, &cur, day)
            .zip(per_usd(&rates, display.as_str(), day))
            .and_then(|(f, t)| {
                Some((
                    bf_core::convert_cents(rev, f, t).ok()?,
                    bf_core::convert_cents(com, f, t).ok()?,
                ))
            });
        match converted {
            Some((r, c)) => {
                revenue += i128::from(r);
                commission += i128::from(c);
            }
            None => match left.last_mut().filter(|u| u.program_id == program) {
                Some(u) => {
                    u.revenue_cents = u.revenue_cents.saturating_add(rev);
                    u.commission_cents = u.commission_cents.saturating_add(com);
                }
                None => left.push(Unconverted {
                    program_id: program,
                    currency: cur,
                    revenue_cents: rev,
                    commission_cents: com,
                }),
            },
        }
    }
    let clamp = |v: i128| i64::try_from(v).map_err(|_| AppError::Internal);
    Ok(Json(SummaryAll {
        display_currency: display.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        revenue_cents: clamp(revenue)?,
        commission_cents: clamp(commission)?,
        unconverted: left,
    }))
}

// ---- per-code stats (C30)

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Day,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CodeStatsQuery {
    /// First day, `YYYY-MM-DD` UTC, inclusive. Default: 29 days before `to`.
    from: Option<String>,
    /// Last day, `YYYY-MM-DD` UTC, inclusive. Default: today.
    to: Option<String>,
    /// Series bucket; only `day` is supported.
    granularity: Option<Granularity>,
}

#[derive(Serialize, ToSchema)]
pub struct CodeTotals {
    clicks: i64,
    /// Clicks the tracker flagged (a subset of `clicks`).
    flagged_clicks: i64,
    conversions: i64,
    revenue_cents: i64,
    commission_cents: i64,
}

#[derive(Serialize, ToSchema)]
pub struct CodeDay {
    /// `YYYY-MM-DD` (UTC); every day of the range is present.
    day: String,
    clicks: i64,
    conversions: i64,
    commission_cents: i64,
}

#[derive(Serialize, ToSchema)]
pub struct CodeCountry {
    country: String,
    clicks: i64,
}

#[derive(Serialize, ToSchema)]
pub struct CodeReferrer {
    host: String,
    clicks: i64,
}

#[derive(Serialize, ToSchema)]
pub struct CodeStats {
    code: String,
    from: String,
    to: String,
    /// The program's currency; amounts are integer minor units.
    currency: String,
    totals: CodeTotals,
    series: Vec<CodeDay>,
    /// Top 10 by clicks.
    top_countries: Vec<CodeCountry>,
    /// Top 10 by clicks.
    top_referrers: Vec<CodeReferrer>,
}

async fn code_stats_for(
    s: &AppState,
    workspace: Uuid,
    affiliate: Option<Uuid>,
    code: String,
    q: CodeStatsQuery,
) -> Result<Json<CodeStats>, AppError> {
    // Day is the only bucket today; this match stops compiling when another is added.
    match q.granularity.unwrap_or(Granularity::Day) {
        Granularity::Day => {}
    }
    let to =
        q.to.as_deref()
            .map_or_else(|| Ok(Utc::now().date_naive()), parse_day)?;
    let from = q
        .from
        .as_deref()
        .map_or_else(|| Ok(to - Duration::days(29)), parse_day)?;
    if from > to || (to - from).num_days() >= MAX_RANGE_DAYS {
        return Err(AppError::BadRequest(format!(
            "from must not be after to, and the range is at most {MAX_RANGE_DAYS} days"
        )));
    }
    let r = bf_db::code_stats::find_code(&s.pool, workspace, &code, affiliate)
        .await?
        .ok_or(AppError::NotFound("code not found"))?;
    let st = bf_db::code_stats::code_stats(&s.pool, &code, &r, from, to).await?;
    Ok(Json(CodeStats {
        code,
        from: from.to_string(),
        to: to.to_string(),
        currency: r.currency,
        totals: CodeTotals {
            clicks: st.totals.clicks,
            flagged_clicks: st.totals.flagged_clicks,
            conversions: st.totals.conversions,
            revenue_cents: st.totals.revenue_cents,
            commission_cents: st.totals.commission_cents,
        },
        series: st
            .series
            .into_iter()
            .map(|d| CodeDay {
                day: d.day.to_string(),
                clicks: d.clicks,
                conversions: d.conversions,
                commission_cents: d.commission_cents,
            })
            .collect(),
        top_countries: st
            .top_countries
            .into_iter()
            .map(|c| CodeCountry {
                country: c.country,
                clicks: c.clicks,
            })
            .collect(),
        top_referrers: st
            .top_referrers
            .into_iter()
            .map(|c| CodeReferrer {
                host: c.host,
                clicks: c.clicks,
            })
            .collect(),
    }))
}

/// Stats of one referral code in the workspace (live from clicks and conversions).
#[utoipa::path(
    get,
    path = "/api/v1/referral-codes/{code}/stats",
    tag = "analytics",
    security(("cookie" = [])),
    params(("code" = String, Path, description = "Referral code"), CodeStatsQuery),
    responses(
        (status = 200, body = CodeStats),
        (status = 400, body = ErrorBody),
        (status = 404, description = "Unknown code or code of another workspace", body = ErrorBody),
    )
)]
pub async fn code_stats(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(code): Path<String>,
    Query(q): Query<CodeStatsQuery>,
) -> Result<Json<CodeStats>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    code_stats_for(&s, ws, None, code, q).await
}

/// Stats of one of the signed-in affiliate's own referral codes.
#[utoipa::path(
    get,
    path = "/api/v1/me/referral-codes/{code}/stats",
    tag = "me",
    security(("cookie" = [])),
    params(("code" = String, Path, description = "Referral code"), CodeStatsQuery),
    responses(
        (status = 200, body = CodeStats),
        (status = 400, body = ErrorBody),
        (status = 404, description = "Unknown code or not the caller's", body = ErrorBody),
    )
)]
pub async fn me_code_stats(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    Path(code): Path<String>,
    Query(q): Query<CodeStatsQuery>,
) -> Result<Json<CodeStats>, AppError> {
    let (ws, own) = viewer(&s, &auth.principal).await?;
    code_stats_for(&s, ws, own, code, q).await
}
