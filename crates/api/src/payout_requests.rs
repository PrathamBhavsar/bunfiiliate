//! Affiliate payout requests: `pending -> approved -> paid`, or `pending -> rejected`.
//!
//! A request only asks. The payout is sized from the ledger when an admin approves (anti-pattern
//! #26): the affiliate's current payable net of open payouts, never the requested amount, so a
//! chargeback between request and decision cannot be overpaid. Approval drafts a one-line batch
//! that follows the normal batch flow.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post as post_route};
use axum::{Json, Router};
use bf_db::payouts as db;
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
use crate::payouts::insert_payout;
use crate::programs::viewer;

const MAX_REASON: usize = 500;

/// Notify every workspace admin, inside the caller's transaction.
async fn notify_admins(
    c: &mut PgConnection,
    ws: Uuid,
    kind: &str,
    body: &str,
) -> Result<(), AppError> {
    let admins: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE workspace_id = $1 AND role IN ('admin', 'super_admin')",
    )
    .bind(ws)
    .fetch_all(&mut *c)
    .await?;
    for a in admins {
        bf_db::auth::insert_notification(&mut *c, a, kind, body).await?;
    }
    Ok(())
}

/// File a request for `affiliate`. The caller has resolved the affiliate from the session.
pub async fn create_request(
    pool: &PgPool,
    ws: Uuid,
    affiliate: Uuid,
    program: Uuid,
    requested_cents: i64,
    currency: &str,
) -> Result<Uuid, AppError> {
    if requested_cents <= 0 {
        return Err(AppError::BadRequest(
            "requested_cents must be positive".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let prog: Option<(String,)> = sqlx::query_as(
        "SELECT pr.currency::text FROM programs pr JOIN enrollments e ON e.program_id = pr.id \
         WHERE pr.id = $1 AND pr.workspace_id = $2 AND e.affiliate_id = $3",
    )
    .bind(program)
    .bind(ws)
    .bind(affiliate)
    .fetch_optional(&mut *tx)
    .await?;
    let (pc,) = prog.ok_or(AppError::NotFound("program not found"))?;
    if !pc.eq_ignore_ascii_case(currency) {
        return Err(AppError::BadRequest(
            "currency does not match the program".into(),
        ));
    }
    let min: i64 = sqlx::query_scalar("SELECT payout_min_cents FROM affiliates WHERE id = $1")
        .bind(affiliate)
        .fetch_one(&mut *tx)
        .await?;
    if requested_cents < min {
        return Err(AppError::BadRequest("below the minimum payout".into()));
    }
    let owed = db::owed_for(&mut tx, program, affiliate)
        .await?
        .unwrap_or(0);
    if requested_cents > owed {
        return Err(AppError::BadRequest("exceeds the payable balance".into()));
    }
    let id = Uuid::now_v7();
    // The partial unique index makes a second concurrent insert fail: 409 via `From<sqlx::Error>`.
    sqlx::query(
        "INSERT INTO payout_requests (id, affiliate_id, program_id, requested_cents, currency) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(affiliate)
    .bind(program)
    .bind(requested_cents)
    .bind(pc.to_uppercase())
    .execute(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            AppError::Conflict("a payout request is already pending")
        }
        _ => e.into(),
    })?;
    notify_admins(
        &mut tx,
        ws,
        "payout_request.created",
        "An affiliate requested a payout",
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// Locked pending request: `(affiliate, program, requested, currency, affiliate user)`.
struct Pending {
    affiliate: Uuid,
    program: Uuid,
    requested: i64,
    currency: String,
    user: Uuid,
}

async fn lock_pending(c: &mut PgConnection, ws: Uuid, id: Uuid) -> Result<Pending, AppError> {
    let row: Option<(Uuid, Uuid, i64, String, String, Uuid)> = sqlx::query_as(
        "SELECT r.affiliate_id, r.program_id, r.requested_cents, r.currency::text, \
                r.status::text, a.user_id \
         FROM payout_requests r JOIN programs pr ON pr.id = r.program_id \
           JOIN affiliates a ON a.id = r.affiliate_id \
         WHERE r.id = $1 AND pr.workspace_id = $2 FOR UPDATE OF r",
    )
    .bind(id)
    .bind(ws)
    .fetch_optional(&mut *c)
    .await?;
    let (affiliate, program, requested, currency, st, user) =
        row.ok_or(AppError::NotFound("payout request not found"))?;
    if st != "pending" {
        return Err(AppError::Conflict("payout request is already decided"));
    }
    Ok(Pending {
        affiliate,
        program,
        requested,
        currency,
        user,
    })
}

/// Approve: lock program and affiliate, size from the current balance, draft a one-line batch.
/// Returns the payout amount.
pub async fn approve_request(
    pool: &PgPool,
    ws: Uuid,
    id: Uuid,
    admin: Uuid,
) -> Result<i64, AppError> {
    let mut tx = pool.begin().await?;
    let r = lock_pending(&mut tx, ws, id).await?;
    // Same lock as batch creation, so a batch cannot claim this payable concurrently; the
    // affiliate row lock serializes concurrent approvals for one affiliate.
    let cur: String =
        sqlx::query_scalar("SELECT currency::text FROM programs WHERE id = $1 FOR NO KEY UPDATE")
            .bind(r.program)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query_scalar::<_, i32>("SELECT 1 FROM affiliates WHERE id = $1 FOR NO KEY UPDATE")
        .bind(r.affiliate)
        .fetch_one(&mut *tx)
        .await?;
    if !cur.eq_ignore_ascii_case(&r.currency) {
        return Err(AppError::Conflict(
            "program currency changed since the request",
        ));
    }
    let owed = db::owed_for(&mut tx, r.program, r.affiliate)
        .await?
        .unwrap_or(0);
    let min: i64 = sqlx::query_scalar("SELECT payout_min_cents FROM affiliates WHERE id = $1")
        .bind(r.affiliate)
        .fetch_one(&mut *tx)
        .await?;
    if owed <= 0 || owed < min {
        return Err(AppError::Conflict("nothing payable at the minimum"));
    }
    let batch = Uuid::now_v7();
    sqlx::query("INSERT INTO payout_batches (id, program_id, created_by) VALUES ($1, $2, $3)")
        .bind(batch)
        .bind(r.program)
        .bind(admin)
        .execute(&mut *tx)
        .await?;
    let payout = insert_payout(&mut tx, ws, batch, r.program, r.affiliate, owed).await?;
    sqlx::query(
        "UPDATE payout_requests SET status = 'approved', payout_id = $2, decided_at = now(), \
         decided_by = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(payout)
    .bind(admin)
    .execute(&mut *tx)
    .await?;
    bf_db::auth::insert_notification(
        &mut *tx,
        r.user,
        "payout_request.approved",
        &format!(
            "Your payout request was approved ({} requested)",
            r.requested
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(owed)
}

pub async fn reject_request(
    pool: &PgPool,
    ws: Uuid,
    id: Uuid,
    admin: Uuid,
    reason: &str,
) -> Result<(), AppError> {
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_REASON {
        return Err(AppError::BadRequest(
            "reason is required (max 500 characters)".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let r = lock_pending(&mut tx, ws, id).await?;
    sqlx::query(
        "UPDATE payout_requests SET status = 'rejected', rejection_reason = $2, \
         decided_at = now(), decided_by = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(reason)
    .bind(admin)
    .execute(&mut *tx)
    .await?;
    bf_db::auth::insert_notification(
        &mut *tx,
        r.user,
        "payout_request.rejected",
        &format!("Your payout request was rejected: {reason}"),
    )
    .await?;
    Ok(tx.commit().await?)
}

// ---- routes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me/payout-requests", get(me_list).post(me_create))
        .route("/api/v1/payout-requests", get(admin_list))
        .route("/api/v1/payout-requests/{id}/approve", post_route(approve))
        .route("/api/v1/payout-requests/{id}/reject", post_route(reject))
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct RequestView {
    id: Uuid,
    affiliate_id: Uuid,
    program_id: Uuid,
    requested_cents: i64,
    currency: String,
    /// `pending`, `approved`, `rejected` or `paid`.
    status: String,
    rejection_reason: Option<String>,
    payout_id: Option<Uuid>,
    created_at: String,
    decided_at: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct NewRequest {
    program_id: Uuid,
    requested_cents: i64,
    currency: String,
}

#[derive(Deserialize, ToSchema)]
pub struct RejectBody {
    reason: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RequestFilter {
    program_id: Option<Uuid>,
    /// `pending`, `approved`, `rejected` or `paid`.
    status: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct Approved {
    amount_cents: i64,
}

fn ts(col: &str) -> String {
    format!("to_char({col} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')")
}

fn select() -> String {
    format!(
        "SELECT r.id, r.affiliate_id, r.program_id, r.requested_cents, r.currency::text AS currency, \
                r.status::text AS status, r.rejection_reason, r.payout_id, {} AS created_at, \
                {} AS decided_at \
         FROM payout_requests r JOIN programs pr ON pr.id = r.program_id \
         WHERE pr.workspace_id = $1 AND ($2::uuid IS NULL OR r.affiliate_id = $2) \
           AND ($3::uuid IS NULL OR r.program_id = $3) \
           AND ($4::text IS NULL OR r.status = $4::payout_request_status)",
        ts("r.created_at"),
        ts("r.decided_at")
    )
}

async fn list(
    pool: &PgPool,
    ws: Uuid,
    affiliate: Option<Uuid>,
    f: &RequestFilter,
    page: &PageQuery,
) -> Result<Paged<RequestView>, AppError> {
    if f.status
        .as_deref()
        .is_some_and(|s| !["pending", "approved", "rejected", "paid"].contains(&s))
    {
        return Err(AppError::BadRequest("unknown status".into()));
    }
    let base = select();
    let items = sqlx::query_as(&format!(
        "{base} ORDER BY r.created_at DESC, r.id DESC LIMIT $5 OFFSET $6"
    ))
    .bind(ws)
    .bind(affiliate)
    .bind(f.program_id)
    .bind(f.status.as_deref())
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar(
        "SELECT count(*) FROM payout_requests r JOIN programs pr ON pr.id = r.program_id \
         WHERE pr.workspace_id = $1 AND ($2::uuid IS NULL OR r.affiliate_id = $2) \
           AND ($3::uuid IS NULL OR r.program_id = $3) \
           AND ($4::text IS NULL OR r.status = $4::payout_request_status)",
    )
    .bind(ws)
    .bind(affiliate)
    .bind(f.program_id)
    .bind(f.status.as_deref())
    .fetch_one(pool)
    .await?;
    Ok(page.wrap((items, total)))
}

/// File a payout request for the signed-in affiliate.
#[utoipa::path(
    post,
    path = "/api/v1/me/payout-requests",
    tag = "me",
    security(("cookie" = [])),
    request_body = NewRequest,
    responses(
        (status = 201, body = RequestView),
        (status = 400, description = "Over balance, under minimum or wrong currency", body = ErrorBody),
        (status = 404, description = "Not enrolled", body = ErrorBody),
        (status = 409, description = "A request is already pending", body = ErrorBody),
    )
)]
pub async fn me_create(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    AppJson(req): AppJson<NewRequest>,
) -> Result<(StatusCode, Json<RequestView>), AppError> {
    let (ws, own) = viewer(&s, &auth.principal).await?;
    let own = own.ok_or(AppError::Forbidden("no affiliate profile"))?;
    let id = create_request(
        &s.pool,
        ws,
        own,
        req.program_id,
        req.requested_cents,
        &req.currency,
    )
    .await?;
    let view = sqlx::query_as(&format!("{} AND r.id = $5", select()))
        .bind(ws)
        .bind(own)
        .bind(None::<Uuid>)
        .bind(None::<&str>)
        .bind(id)
        .fetch_one(&s.pool)
        .await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// The signed-in affiliate's own requests, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/me/payout-requests",
    tag = "me",
    security(("cookie" = [])),
    params(RequestFilter, PageQuery),
    responses((status = 200, body = Paged<RequestView>), (status = 403, body = ErrorBody))
)]
pub async fn me_list(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    Query(f): Query<RequestFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<RequestView>>, AppError> {
    let (ws, own) = viewer(&s, &auth.principal).await?;
    let own = own.ok_or(AppError::Forbidden("no affiliate profile"))?;
    Ok(Json(list(&s.pool, ws, Some(own), &f, &page).await?))
}

/// Workspace payout requests, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/payout-requests",
    tag = "payouts",
    security(("cookie" = [])),
    params(RequestFilter, PageQuery),
    responses((status = 200, body = Paged<RequestView>), (status = 400, body = ErrorBody))
)]
pub async fn admin_list(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<RequestFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<RequestView>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    Ok(Json(list(&s.pool, ws, None, &f, &page).await?))
}

/// Approve: drafts a payout sized from the current balance.
#[utoipa::path(
    post,
    path = "/api/v1/payout-requests/{id}/approve",
    tag = "payouts",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "RequestView id")),
    responses(
        (status = 200, body = Approved),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Decided already, or nothing payable", body = ErrorBody),
    )
)]
pub async fn approve(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Approved>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let amount_cents = approve_request(&s.pool, ws, id, auth.principal.user_id).await?;
    audit::record(
        &s.pool,
        &auth.principal,
        "payout_request.approve",
        "payout_request",
        id,
        Some(json!({ "amount_cents": amount_cents })),
    )
    .await?;
    Ok(Json(Approved { amount_cents }))
}

/// Reject with a reason shown to the affiliate.
#[utoipa::path(
    post,
    path = "/api/v1/payout-requests/{id}/reject",
    tag = "payouts",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "RequestView id")),
    request_body = RejectBody,
    responses(
        (status = 204),
        (status = 400, description = "Missing reason", body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Decided already", body = ErrorBody),
    )
)]
pub async fn reject(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<RejectBody>,
) -> Result<StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    reject_request(&s.pool, ws, id, auth.principal.user_id, &req.reason).await?;
    audit::record(
        &s.pool,
        &auth.principal,
        "payout_request.reject",
        "payout_request",
        id,
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
