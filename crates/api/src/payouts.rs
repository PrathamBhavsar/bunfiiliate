//! Payout batches: `draft -> approved -> paid`, or `draft -> cancelled`.
//!
//! Creating a batch posts nothing: money only moves at `mark_paid` (payable -> cash).
//! Each payout pays the affiliate's whole unbatched payable, so clawbacks from reversed paid
//! conversions are netted automatically.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::routing::{get, post as post_route};
use axum::{Json, Router};
use bf_core::{Currency, Money, PayoutEvent, PayoutStatus};
use bf_db::payouts::{self, CsvRow};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Affiliate as AffiliateRole, Auth};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::page::{PageQuery, Paged};
use crate::programs::{require_program, viewer};
use crate::worker::post;

/// Locked batch row.
struct Batch {
    status: PayoutStatus,
    currency: Currency,
}

fn status(s: &str) -> Option<PayoutStatus> {
    Some(match s {
        "draft" => PayoutStatus::Draft,
        "approved" => PayoutStatus::Approved,
        "paid" => PayoutStatus::Paid,
        "cancelled" => PayoutStatus::Cancelled,
        _ => return None,
    })
}

fn invalid<T>(_: T) -> AppError {
    AppError::Conflict("payout batch cannot take this action")
}

async fn lock(c: &mut PgConnection, id: Uuid) -> Result<Batch, AppError> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT b.status::text, p.currency::text FROM payout_batches b \
         JOIN programs p ON p.id = b.program_id WHERE b.id = $1 FOR UPDATE OF b",
    )
    .bind(id)
    .fetch_optional(&mut *c)
    .await?;
    let (st, cur) = row.ok_or(AppError::NotFound("payout batch not found"))?;
    match (status(&st), Currency::new(&cur)) {
        (Some(status), Some(currency)) => Ok(Batch { status, currency }),
        _ => Err(AppError::Internal),
    }
}

/// Draft one payout per affiliate owed at least their minimum, and claim their approved
/// conversions. `None` when nobody is owed.
pub async fn create_batch(
    pool: &PgPool,
    program_id: Uuid,
    created_by: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let mut tx = pool.begin().await?;
    // NO KEY UPDATE keeps ingest (FK checks) running but serializes batch creation, so two
    // callers cannot both claim the same payable.
    sqlx::query_scalar::<_, i32>("SELECT 1 FROM programs WHERE id = $1 FOR NO KEY UPDATE")
        .bind(program_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound("program not found"))?;
    let owed = payouts::owed(&mut tx, program_id).await?;
    if owed.is_empty() {
        return Ok(None);
    }
    let batch = Uuid::now_v7();
    sqlx::query("INSERT INTO payout_batches (id, program_id, created_by) VALUES ($1, $2, $3)")
        .bind(batch)
        .bind(program_id)
        .bind(created_by)
        .execute(&mut *tx)
        .await?;
    for o in owed {
        let payout = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO payouts (id, batch_id, affiliate_id, amount_cents) VALUES ($1, $2, $3, $4)",
        )
        .bind(payout)
        .bind(batch)
        .bind(o.affiliate_id)
        .bind(o.amount_cents)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE conversions SET payout_id = $1 \
             WHERE program_id = $2 AND affiliate_id = $3 AND status = 'approved' AND payout_id IS NULL",
        )
        .bind(payout)
        .bind(program_id)
        .bind(o.affiliate_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Some(batch))
}

pub async fn approve_batch(pool: &PgPool, batch: Uuid, approved_by: Uuid) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let b = lock(&mut tx, batch).await?;
    b.status.transition(PayoutEvent::Approve).map_err(invalid)?;
    sqlx::query("UPDATE payout_batches SET status = 'approved', approved_by = $2 WHERE id = $1")
        .bind(batch)
        .bind(approved_by)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE payouts SET status = 'approved' WHERE batch_id = $1")
        .bind(batch)
        .execute(&mut *tx)
        .await?;
    Ok(tx.commit().await?)
}

/// Only a draft batch can be cancelled; its conversions go back to the payable pool.
pub async fn cancel_batch(pool: &PgPool, batch: Uuid) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let b = lock(&mut tx, batch).await?;
    b.status.transition(PayoutEvent::Cancel).map_err(invalid)?;
    sqlx::query("UPDATE payout_batches SET status = 'cancelled' WHERE id = $1")
        .bind(batch)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE payouts SET status = 'cancelled' WHERE batch_id = $1")
        .bind(batch)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE conversions SET payout_id = NULL \
         WHERE payout_id IN (SELECT id FROM payouts WHERE batch_id = $1)",
    )
    .bind(batch)
    .execute(&mut *tx)
    .await?;
    Ok(tx.commit().await?)
}

/// Pay an approved batch: post payable -> cash per payout and turn its claimed conversions
/// `paid`. A second call finds the batch paid and is rejected.
pub async fn mark_paid(pool: &PgPool, batch: Uuid) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let b = lock(&mut tx, batch).await?;
    // The batch itself moves no money; its payouts do.
    b.status
        .transition(PayoutEvent::MarkPaid(Money::new(0, b.currency)))
        .map_err(invalid)?;
    let rows: Vec<(Uuid, Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, affiliate_id, amount_cents, status::text FROM payouts \
         WHERE batch_id = $1 ORDER BY id FOR UPDATE",
    )
    .bind(batch)
    .fetch_all(&mut *tx)
    .await?;
    let program: Uuid = sqlx::query_scalar("SELECT program_id FROM payout_batches WHERE id = $1")
        .bind(batch)
        .fetch_one(&mut *tx)
        .await?;
    for (payout, affiliate, cents, st) in rows {
        let st = status(&st).ok_or(AppError::Internal)?;
        let (_, txn) = st
            .transition(PayoutEvent::MarkPaid(Money::new(cents, b.currency)))
            .map_err(invalid)?;
        let txn_id = match txn {
            Some(t) => post(&mut tx, &t, program, affiliate, (None, Some(payout))).await?,
            None => None,
        };
        sqlx::query("UPDATE payouts SET status = 'paid', ledger_txn_id = $2 WHERE id = $1")
            .bind(payout)
            .bind(txn_id)
            .execute(&mut *tx)
            .await?;
        // Approved -> Paid; a conversion reversed since the batch was drafted stays reversed.
        sqlx::query(
            "UPDATE conversions SET status = 'paid' WHERE payout_id = $1 AND status = 'approved'",
        )
        .bind(payout)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE payout_batches SET status = 'paid', paid_at = now() WHERE id = $1")
        .bind(batch)
        .execute(&mut *tx)
        .await?;
    Ok(tx.commit().await?)
}

/// CSV of a batch for the bank/payout tool.
pub async fn batch_csv(pool: &PgPool, batch: Uuid) -> Result<String, AppError> {
    let mut conn = pool.acquire().await?;
    sqlx::query_scalar::<_, i32>("SELECT 1 FROM payout_batches WHERE id = $1")
        .bind(batch)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(AppError::NotFound("payout batch not found"))?;
    Ok(export_csv(&payouts::batch_rows(&mut conn, batch).await?))
}

/// `email,amount,currency` with a header; amounts are decimal with two places.
pub fn export_csv(rows: &[CsvRow]) -> String {
    let mut out = String::from("email,amount,currency\n");
    for r in rows {
        out.push_str(&format!(
            "{},{}.{:02},{}\n",
            field(&r.email),
            r.amount_cents / 100,
            r.amount_cents % 100,
            field(&r.currency)
        ));
    }
    out
}

/// RFC 4180 quoting, only when needed. A leading `= + - @` (after any whitespace or control characters) is defused with a
/// quote so a spreadsheet never reads an affiliate-controlled email as a formula.
fn field(s: &str) -> String {
    let lead = s.trim_start_matches(|c: char| c.is_whitespace() || c.is_control());
    let s = if lead.starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s.to_owned()
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

// ---- routes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/payouts/batches",
            get(list_batches).post(create_batch_route),
        )
        .route("/api/v1/payouts/batches/{id}", get(get_batch))
        .route("/api/v1/payouts/batches/{id}/approve", post_route(approve))
        .route(
            "/api/v1/payouts/batches/{id}/mark-paid",
            post_route(mark_paid_route),
        )
        .route("/api/v1/payouts/batches/{id}/cancel", post_route(cancel))
        .route("/api/v1/payouts/batches/{id}/export.csv", get(export))
        .route("/api/v1/me/payouts", get(me_payouts))
}

/// `timestamptz` column as RFC 3339 UTC text.
fn ts(col: &str) -> String {
    format!("to_char({col} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')")
}

#[derive(Deserialize, ToSchema)]
pub struct NewBatch {
    program_id: Uuid,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BatchFilter {
    program_id: Option<Uuid>,
    /// `draft`, `approved`, `paid` or `cancelled`.
    status: Option<String>,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct BatchView {
    id: Uuid,
    program_id: Uuid,
    /// `draft`, `approved`, `paid` or `cancelled`.
    status: String,
    currency: String,
    created_by: Uuid,
    approved_by: Option<Uuid>,
    created_at: String,
    paid_at: Option<String>,
    payout_count: i64,
    total_cents: i64,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Payout {
    id: Uuid,
    affiliate_id: Uuid,
    /// Payout email, else the affiliate's account email.
    email: String,
    amount_cents: i64,
    /// `draft`, `approved`, `paid` or `cancelled`.
    status: String,
    /// The `AffPayable -> Cash` ledger txn, once paid.
    ledger_txn_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub struct BatchDetail {
    #[serde(flatten)]
    batch: BatchView,
    payouts: Vec<Payout>,
}

/// `$1` workspace, `$2` program, `$3` status.
const BATCHES: &str = "payout_batches b JOIN programs pr ON pr.id = b.program_id \
    WHERE pr.workspace_id = $1 AND ($2::uuid IS NULL OR b.program_id = $2) \
      AND ($3::text IS NULL OR b.status = $3::payout_status)";

fn batch_select() -> String {
    format!(
        "SELECT b.id, b.program_id, b.status::text AS status, pr.currency::text AS currency, \
                b.created_by, b.approved_by, {} AS created_at, {} AS paid_at, \
                (SELECT count(*) FROM payouts p WHERE p.batch_id = b.id) AS payout_count, \
                (SELECT COALESCE(sum(p.amount_cents), 0) FROM payouts p WHERE p.batch_id = b.id)::bigint \
                  AS total_cents \
         FROM ",
        ts("b.created_at"),
        ts("b.paid_at")
    )
}

async fn detail(pool: &PgPool, ws: Uuid, id: Uuid) -> Result<BatchDetail, AppError> {
    let batch = sqlx::query_as(&format!("{}{BATCHES} AND b.id = $4", batch_select()))
        .bind(ws)
        .bind(None::<Uuid>)
        .bind(None::<&str>)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("payout batch not found"))?;
    let payouts = sqlx::query_as(
        "SELECT p.id, p.affiliate_id, COALESCE(a.payout_email, u.email)::text AS email, \
                p.amount_cents, p.status::text AS status, p.ledger_txn_id \
         FROM payouts p JOIN affiliates a ON a.id = p.affiliate_id JOIN users u ON u.id = a.user_id \
         WHERE p.batch_id = $1 ORDER BY 3, p.id",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(BatchDetail { batch, payouts })
}

/// `404` unless the batch belongs to the workspace; run before any state change.
async fn require_batch(pool: &PgPool, ws: Uuid, id: Uuid) -> Result<(), AppError> {
    sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM payout_batches b JOIN programs pr ON pr.id = b.program_id \
         WHERE b.id = $1 AND pr.workspace_id = $2",
    )
    .bind(id)
    .bind(ws)
    .fetch_optional(pool)
    .await?
    .map(|_| ())
    .ok_or(AppError::NotFound("payout batch not found"))
}

/// Draft a batch paying every active affiliate of the program who is owed at least their
/// minimum. `409` when nobody is.
#[utoipa::path(
    post,
    path = "/api/v1/payouts/batches",
    tag = "payouts",
    security(("cookie" = [])),
    request_body = NewBatch,
    responses(
        (status = 201, body = BatchDetail),
        (status = 404, description = "Unknown program", body = ErrorBody),
        (status = 409, description = "Nobody is owed a payout", body = ErrorBody),
    )
)]
pub async fn create_batch_route(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<NewBatch>,
) -> Result<(StatusCode, Json<BatchDetail>), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    require_program(&mut *s.pool.acquire().await?, ws, req.program_id).await?;
    let id = create_batch(&s.pool, req.program_id, auth.principal.user_id)
        .await?
        .ok_or(AppError::Conflict("nobody is owed a payout"))?;
    audit::record(
        &s.pool,
        &auth.principal,
        "payout_batch.create",
        "payout_batch",
        id,
        Some(json!({ "program_id": req.program_id })),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(detail(&s.pool, ws, id).await?)))
}

/// List batches, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/payouts/batches",
    tag = "payouts",
    security(("cookie" = [])),
    params(BatchFilter, PageQuery),
    responses((status = 200, body = Paged<BatchView>), (status = 400, body = ErrorBody))
)]
pub async fn list_batches(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<BatchFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<BatchView>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    if f.status.as_deref().is_some_and(|st| status(st).is_none()) {
        return Err(AppError::BadRequest("unknown status".into()));
    }
    let items = sqlx::query_as(&format!(
        "{}{BATCHES} ORDER BY b.created_at DESC, b.id DESC LIMIT $4 OFFSET $5",
        batch_select()
    ))
    .bind(ws)
    .bind(f.program_id)
    .bind(f.status.as_deref())
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(&s.pool)
    .await?;
    let total = sqlx::query_scalar(&format!("SELECT count(*) FROM {BATCHES}"))
        .bind(ws)
        .bind(f.program_id)
        .bind(f.status.as_deref())
        .fetch_one(&s.pool)
        .await?;
    Ok(Json(page.wrap((items, total))))
}

/// One batch with its payouts.
#[utoipa::path(
    get,
    path = "/api/v1/payouts/batches/{id}",
    tag = "payouts",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "BatchView id")),
    responses((status = 200, body = BatchDetail), (status = 404, body = ErrorBody))
)]
pub async fn get_batch(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<BatchDetail>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    Ok(Json(detail(&s.pool, ws, id).await?))
}

/// `draft -> approved`.
#[utoipa::path(
    post,
    path = "/api/v1/payouts/batches/{id}/approve",
    tag = "payouts",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "BatchView id")),
    responses(
        (status = 200, body = BatchDetail),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Not a draft", body = ErrorBody),
    )
)]
pub async fn approve(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<BatchDetail>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    require_batch(&s.pool, ws, id).await?;
    approve_batch(&s.pool, id, auth.principal.user_id).await?;
    audit::record(
        &s.pool,
        &auth.principal,
        "payout_batch.approve",
        "payout_batch",
        id,
        None,
    )
    .await?;
    Ok(Json(detail(&s.pool, ws, id).await?))
}

/// `approved -> paid`: posts payable -> cash per payout.
#[utoipa::path(
    post,
    path = "/api/v1/payouts/batches/{id}/mark-paid",
    tag = "payouts",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "BatchView id")),
    responses(
        (status = 200, body = BatchDetail),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Not approved", body = ErrorBody),
    )
)]
pub async fn mark_paid_route(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<BatchDetail>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    require_batch(&s.pool, ws, id).await?;
    mark_paid(&s.pool, id).await?;
    audit::record(
        &s.pool,
        &auth.principal,
        "payout_batch.mark_paid",
        "payout_batch",
        id,
        None,
    )
    .await?;
    Ok(Json(detail(&s.pool, ws, id).await?))
}

/// `draft -> cancelled`: releases the batch's conversions.
#[utoipa::path(
    post,
    path = "/api/v1/payouts/batches/{id}/cancel",
    tag = "payouts",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "BatchView id")),
    responses(
        (status = 200, body = BatchDetail),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Not a draft", body = ErrorBody),
    )
)]
pub async fn cancel(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<BatchDetail>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    require_batch(&s.pool, ws, id).await?;
    cancel_batch(&s.pool, id).await?;
    audit::record(
        &s.pool,
        &auth.principal,
        "payout_batch.cancel",
        "payout_batch",
        id,
        None,
    )
    .await?;
    Ok(Json(detail(&s.pool, ws, id).await?))
}

/// `email,amount,currency` CSV for the bank or payout tool.
#[utoipa::path(
    get,
    path = "/api/v1/payouts/batches/{id}/export.csv",
    tag = "payouts",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "BatchView id")),
    responses(
        (status = 200, description = "text/csv", content_type = "text/csv", body = String),
        (status = 404, body = ErrorBody),
    )
)]
pub async fn export(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<([(axum::http::HeaderName, String); 2], String), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    require_batch(&s.pool, ws, id).await?;
    Ok((
        [
            (CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"payouts-{id}.csv\""),
            ),
        ],
        batch_csv(&s.pool, id).await?,
    ))
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct MyPayout {
    id: Uuid,
    batch_id: Uuid,
    program_id: Uuid,
    amount_cents: i64,
    currency: String,
    /// `approved` (owed, not yet sent) or `paid`.
    status: String,
    paid_at: Option<String>,
}

/// `$1` workspace, `$2` affiliate. Drafts and cancelled payouts are internal.
const MY_PAYOUTS: &str = "payouts p JOIN payout_batches b ON b.id = p.batch_id \
    JOIN programs pr ON pr.id = b.program_id \
    WHERE pr.workspace_id = $1 AND p.affiliate_id = $2 AND p.status IN ('approved', 'paid')";

/// The signed-in affiliate's approved and paid payouts, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/me/payouts",
    tag = "me",
    security(("cookie" = [])),
    params(PageQuery),
    responses((status = 200, body = Paged<MyPayout>), (status = 403, body = ErrorBody))
)]
pub async fn me_payouts(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<MyPayout>>, AppError> {
    let (ws, own) = viewer(&s, &auth.principal).await?;
    let own = own.ok_or(AppError::Forbidden("no affiliate profile"))?;
    let items = sqlx::query_as(&format!(
        "SELECT p.id, p.batch_id, b.program_id, p.amount_cents, pr.currency::text AS currency, \
                p.status::text AS status, {} AS paid_at \
         FROM {MY_PAYOUTS} ORDER BY b.created_at DESC, p.id DESC LIMIT $3 OFFSET $4",
        ts("b.paid_at")
    ))
    .bind(ws)
    .bind(own)
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(&s.pool)
    .await?;
    let total = sqlx::query_scalar(&format!("SELECT count(*) FROM {MY_PAYOUTS}"))
        .bind(ws)
        .bind(own)
        .fetch_one(&s.pool)
        .await?;
    Ok(Json(page.wrap((items, total))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_only_when_needed() {
        let row = |email: &str, cents| CsvRow {
            email: email.into(),
            amount_cents: cents,
            currency: "USD".into(),
        };
        assert_eq!(
            export_csv(&[row("a@x.io", 5), row("b,\"c\"@x.io", 123_456)]),
            "email,amount,currency\na@x.io,0.05,USD\n\"b,\"\"c\"\"@x.io\",1234.56,USD\n"
        );
        assert_eq!(
            export_csv(&[
                row("=HYPERLINK(\"x\")@x.io", 100),
                row("@a.io", 100),
                row(" \t=1+1", 100)
            ]),
            "email,amount,currency\n\"'=HYPERLINK(\"\"x\"\")@x.io\",1.00,USD\n'@a.io,1.00,USD\n' \t=1+1,1.00,USD\n"
        );
    }
}
