//! B05: the affiliate's tax form (`/me/tax-form`) and the admin's 1099-style CSV.
//!
//! Only the last 4 digits of a tax id are ever accepted or stored. This is bookkeeping, not tax
//! advice: self-hosters who file for real export the CSV to their own compliance vendor.

use axum::extract::{Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::{Admin, Affiliate as AffiliateRole, Auth};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::payouts::field;
use crate::programs::viewer;

/// Default for `TAX_REPORT_THRESHOLD_CENTS`: the US 1099-NEC threshold, as a sane default only.
const DEFAULT_THRESHOLD_CENTS: i64 = 60_000;
const MAX_NAME_CHARS: usize = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me/tax-form", get(me_get).put(me_put))
        .route("/api/v1/reports/1099-style.csv", get(report_1099))
}

#[derive(Serialize, ToSchema, sqlx::FromRow, PartialEq, Debug)]
pub struct TaxForm {
    /// `w8`, `w9`, or `none` when nothing was submitted.
    kind: String,
    legal_name: Option<String>,
    /// ISO 3166 alpha-2, upper case.
    country: Option<String>,
    /// Last 4 digits of the tax id; the full id is never stored.
    tax_id_last4: Option<String>,
    /// RFC 3339 UTC.
    submitted_at: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct TaxFormInput {
    /// `w8` or `w9`.
    kind: String,
    legal_name: String,
    /// Two letters.
    country: String,
    /// Exactly 4 digits when present. Anything longer is refused, never truncated.
    tax_id_last4: Option<String>,
}

async fn own(
    s: &AppState,
    auth: &Auth<AffiliateRole>,
) -> Result<(uuid::Uuid, uuid::Uuid), AppError> {
    let (ws, own) = viewer(s, &auth.principal).await?;
    Ok((ws, own.ok_or(AppError::Forbidden("no affiliate profile"))?))
}

/// The signed-in affiliate's tax form.
#[utoipa::path(
    get,
    path = "/api/v1/me/tax-form",
    tag = "me",
    security(("cookie" = [])),
    responses((status = 200, body = TaxForm), (status = 403, body = ErrorBody))
)]
pub async fn me_get(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
) -> Result<Json<TaxForm>, AppError> {
    let (ws, aff) = own(&s, &auth).await?;
    let form = sqlx::query_as(
        "SELECT tax_form_kind::text AS kind, tax_legal_name AS legal_name, \
                tax_country::text AS country, tax_id_last4, \
                to_char(tax_form_submitted_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS submitted_at \
         FROM affiliates WHERE id = $1 AND workspace_id = $2",
    )
        .bind(aff)
        .bind(ws)
        .fetch_one(&s.pool)
        .await?;
    Ok(Json(form))
}

fn bad(msg: &str) -> AppError {
    AppError::BadRequest(msg.into())
}

/// Submit or replace the signed-in affiliate's tax form.
#[utoipa::path(
    put,
    path = "/api/v1/me/tax-form",
    tag = "me",
    security(("cookie" = [])),
    request_body = TaxFormInput,
    responses(
        (status = 200, body = TaxForm),
        (status = 400, description = "Bad kind, name, country or tax id", body = ErrorBody),
        (status = 403, body = ErrorBody),
    )
)]
pub async fn me_put(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    AppJson(req): AppJson<TaxFormInput>,
) -> Result<Json<TaxForm>, AppError> {
    let (ws, aff) = own(&s, &auth).await?;
    if !matches!(req.kind.as_str(), "w8" | "w9") {
        return Err(bad("kind is w8 or w9"));
    }
    let name = req.legal_name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(bad("legal_name is 1 to 200 characters"));
    }
    let country = req.country.to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(bad("country is a 2-letter code"));
    }
    // The value is never echoed: a long one is probably a full tax id.
    if let Some(id) = &req.tax_id_last4
        && !(id.len() == 4 && id.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(bad(
            "tax_id_last4 is exactly 4 digits; never send the full id",
        ));
    }
    sqlx::query(
        "UPDATE affiliates SET tax_form_kind = $3::tax_form_kind, tax_legal_name = $4, \
             tax_country = $5, tax_id_last4 = $6, tax_form_submitted_at = now() \
         WHERE id = $1 AND workspace_id = $2",
    )
    .bind(aff)
    .bind(ws)
    .bind(&req.kind)
    .bind(name)
    .bind(&country)
    .bind(&req.tax_id_last4)
    .execute(&s.pool)
    .await?;
    me_get(State(s), auth).await
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReportQuery {
    /// Calendar year (UTC) of the payout's paid date. Default: the current year.
    year: Option<i32>,
}

/// Paid total per affiliate and currency (currencies are never added together).
#[derive(sqlx::FromRow)]
struct Row {
    legal_name: String,
    country: Option<String>,
    kind: String,
    total: i64,
    currency: String,
}

/// `TAX_REPORT_THRESHOLD_CENTS`, default $600. Unset or unparsable means the default.
fn threshold() -> i64 {
    std::env::var("TAX_REPORT_THRESHOLD_CENTS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(DEFAULT_THRESHOLD_CENTS)
}

/// One row per affiliate and currency whose payouts paid in the year reach the threshold.
#[utoipa::path(
    get,
    path = "/api/v1/reports/1099-style.csv",
    tag = "reports",
    security(("cookie" = [])),
    params(ReportQuery),
    responses(
        (status = 200, description = "text/csv", content_type = "text/csv", body = String),
        (status = 400, body = ErrorBody),
    )
)]
pub async fn report_1099(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(q): Query<ReportQuery>,
) -> Result<([(axum::http::HeaderName, String); 2], String), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let year = q.year.unwrap_or_else(|| Utc::now().year());
    if !(2000..=9999).contains(&year) {
        return Err(bad("year is 2000 to 9999"));
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT COALESCE(a.tax_legal_name, a.payout_email::text, u.email::text) AS legal_name, \
                a.tax_country::text AS country, a.tax_form_kind::text AS kind, \
                sum(p.amount_cents)::bigint AS total, pr.currency::text AS currency \
         FROM payouts p \
         JOIN payout_batches b ON b.id = p.batch_id \
         JOIN programs pr ON pr.id = b.program_id \
         JOIN affiliates a ON a.id = p.affiliate_id \
         JOIN users u ON u.id = a.user_id \
         WHERE pr.workspace_id = $1 AND a.workspace_id = $1 AND p.status = 'paid' \
           AND b.paid_at >= make_timestamptz($2, 1, 1, 0, 0, 0, 'UTC') \
           AND b.paid_at < make_timestamptz($2 + 1, 1, 1, 0, 0, 0, 'UTC') \
         GROUP BY a.id, a.tax_legal_name, a.payout_email, u.email, a.tax_country, a.tax_form_kind, \
                  pr.currency \
         HAVING sum(p.amount_cents) >= $3 \
         ORDER BY legal_name, pr.currency, a.id",
    )
    .bind(ws)
    .bind(year)
    .bind(threshold())
    .fetch_all(&s.pool)
    .await?;
    let mut out = String::from("legal_name,country,tax_form_kind,total_paid_cents,currency\n");
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{}\n",
            field(&r.legal_name),
            field(r.country.as_deref().unwrap_or("")),
            field(&r.kind),
            r.total,
            field(&r.currency)
        ));
    }
    Ok((
        [
            (CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"1099-style-{year}.csv\""),
            ),
        ],
        out,
    ))
}
