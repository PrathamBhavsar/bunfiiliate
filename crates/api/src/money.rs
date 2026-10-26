//! Read side of the money tables (customers, conversions, ledger, affiliate earnings) and the
//! admin conversion actions, which go through `worker::settle` (the `bf_core` state machine).
//!
//! Every query joins `programs` to stay inside the caller's workspace; a foreign id is `404`.
//! Timestamps are RFC 3339 UTC strings.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::analytics::parse_day;
use crate::audit;
use crate::auth::{Admin, Affiliate as AffiliateRole, Any, Auth, Principal};
use crate::error::{AppError, ErrorBody};
use crate::page::{PageQuery, Paged};
use crate::programs::{require_program, viewer};
use crate::worker::{Act, LOCKED_COLS, Locked, settle};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/customers", get(list_customers))
        .route("/api/v1/customers/{id}", get(get_customer))
        .route("/api/v1/conversions", get(list_conversions))
        .route("/api/v1/conversions/{id}", get(get_conversion))
        .route("/api/v1/conversions/{id}/approve", post(approve))
        .route("/api/v1/conversions/{id}/reject", post(reject))
        .route("/api/v1/conversions/{id}/reverse", post(reverse))
        .route("/api/v1/ledger/entries", get(list_entries))
        .route("/api/v1/ledger/balances", get(list_balances))
        .route("/api/v1/me/earnings", get(me_earnings))
}

/// `timestamptz` column as RFC 3339 UTC text.
fn ts(col: &str) -> String {
    format!("to_char({col} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')")
}

async fn count(s: &AppState, sql: String, args: impl FnOnce(Q) -> Q) -> Result<i64, AppError> {
    Ok(args(sqlx::query_scalar(&sql)).fetch_one(&s.pool).await?)
}

type Q<'q> = sqlx::query::QueryScalar<'q, sqlx::Postgres, i64, sqlx::postgres::PgArguments>;

// ---- customers

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CustomerFilter {
    program_id: Option<Uuid>,
    /// The affiliate the customer is attributed to.
    affiliate_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Customer {
    id: Uuid,
    program_id: Uuid,
    /// The customer's id in the merchant's system.
    external_id: String,
    affiliate_id: Option<Uuid>,
    first_click_id: Option<Uuid>,
    attributed_at: Option<String>,
}

/// `$1` workspace, `$2` program, `$3` affiliate.
const CUSTOMERS: &str = "customers cu JOIN programs p ON p.id = cu.program_id \
    WHERE p.workspace_id = $1 AND ($2::uuid IS NULL OR cu.program_id = $2) \
      AND ($3::uuid IS NULL OR cu.affiliate_id = $3)";

fn customer_select() -> String {
    format!(
        "SELECT cu.id, cu.program_id, cu.external_id, cu.affiliate_id, cu.first_click_id, {} AS attributed_at FROM ",
        ts("cu.attributed_at")
    )
}

/// List customers, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/customers",
    tag = "customers",
    security(("cookie" = [])),
    params(CustomerFilter, PageQuery),
    responses((status = 200, body = Paged<Customer>), (status = 403, body = ErrorBody))
)]
pub async fn list_customers(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<CustomerFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Customer>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let items = sqlx::query_as(&format!(
        "{}{CUSTOMERS} ORDER BY cu.id DESC LIMIT $4 OFFSET $5",
        customer_select()
    ))
    .bind(ws)
    .bind(f.program_id)
    .bind(f.affiliate_id)
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(&s.pool)
    .await?;
    let total = count(&s, format!("SELECT count(*) FROM {CUSTOMERS}"), |q| {
        q.bind(ws).bind(f.program_id).bind(f.affiliate_id)
    })
    .await?;
    Ok(Json(page.wrap((items, total))))
}

/// One customer.
#[utoipa::path(
    get,
    path = "/api/v1/customers/{id}",
    tag = "customers",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Customer id")),
    responses((status = 200, body = Customer), (status = 404, body = ErrorBody))
)]
pub async fn get_customer(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Customer>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    sqlx::query_as(&format!("{}{CUSTOMERS} AND cu.id = $4", customer_select()))
        .bind(ws)
        .bind(None::<Uuid>)
        .bind(None::<Uuid>)
        .bind(id)
        .fetch_optional(&s.pool)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound("customer not found"))
}

// ---- conversions

#[derive(Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConversionStatus {
    Pending,
    Approved,
    Rejected,
    Reversed,
    Paid,
}

impl ConversionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Reversed => "reversed",
            Self::Paid => "paid",
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ConversionFilter {
    program_id: Option<Uuid>,
    /// Admins only: one affiliate.
    affiliate_id: Option<Uuid>,
    status: Option<ConversionStatus>,
    /// First day of `occurred_at`, `YYYY-MM-DD` UTC, inclusive.
    from: Option<String>,
    /// Last day of `occurred_at`, `YYYY-MM-DD` UTC, inclusive.
    to: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EarningFilter {
    program_id: Option<Uuid>,
    status: Option<ConversionStatus>,
    from: Option<String>,
    to: Option<String>,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Conversion {
    id: Uuid,
    program_id: Uuid,
    customer_id: Uuid,
    affiliate_id: Uuid,
    conversion_type_id: Option<Uuid>,
    /// The sale's id in the merchant's or payment provider's system.
    external_id: String,
    /// Sale amount in the currency's minor unit.
    amount_cents: i64,
    commission_cents: i64,
    currency: String,
    /// `pending`, `approved`, `rejected`, `reversed` or `paid`.
    status: String,
    occurred_at: String,
    approved_at: Option<String>,
    reversed_at: Option<String>,
    /// The payout that claimed it, once batched.
    payout_id: Option<Uuid>,
}

/// An affiliate's view of a conversion: no customer, no provider ids.
#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Earning {
    id: Uuid,
    program_id: Uuid,
    amount_cents: i64,
    commission_cents: i64,
    currency: String,
    /// `pending`, `approved`, `rejected`, `reversed` or `paid`.
    status: String,
    occurred_at: String,
    approved_at: Option<String>,
}

/// `$1` workspace, `$2` program, `$3` affiliate, `$4` status, `$5` first day, `$6` last day.
const CONVERSIONS: &str = "conversions c JOIN programs p ON p.id = c.program_id \
    WHERE p.workspace_id = $1 AND ($2::uuid IS NULL OR c.program_id = $2) \
      AND ($3::uuid IS NULL OR c.affiliate_id = $3) \
      AND ($4::text IS NULL OR c.status = $4::conversion_status) \
      AND ($5::date IS NULL OR c.occurred_at >= $5::date::timestamp AT TIME ZONE 'UTC') \
      AND ($6::date IS NULL OR c.occurred_at < ($6::date + 1)::timestamp AT TIME ZONE 'UTC')";

struct ConversionScope {
    workspace: Uuid,
    program: Option<Uuid>,
    affiliate: Option<Uuid>,
    status: Option<&'static str>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

impl ConversionScope {
    fn new(
        (workspace, affiliate): (Uuid, Option<Uuid>),
        (program, status): (Option<Uuid>, Option<ConversionStatus>),
        (from, to): (Option<&str>, Option<&str>),
    ) -> Result<Self, AppError> {
        let (from, to) = (
            from.map(parse_day).transpose()?,
            to.map(parse_day).transpose()?,
        );
        if from.zip(to).is_some_and(|(f, t)| f > t) {
            return Err(AppError::BadRequest("from must not be after to".into()));
        }
        Ok(Self {
            workspace,
            program,
            affiliate,
            status: status.map(ConversionStatus::as_str),
            from,
            to,
        })
    }

    async fn list<T>(
        &self,
        s: &AppState,
        cols: &str,
        page: &PageQuery,
    ) -> Result<Paged<T>, AppError>
    where
        T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
    {
        let items = sqlx::query_as(&format!(
            "SELECT {cols} FROM {CONVERSIONS} ORDER BY c.occurred_at DESC, c.id DESC LIMIT $7 OFFSET $8"
        ))
        .bind(self.workspace)
        .bind(self.program)
        .bind(self.affiliate)
        .bind(self.status)
        .bind(self.from)
        .bind(self.to)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(&s.pool)
        .await?;
        let total = count(s, format!("SELECT count(*) FROM {CONVERSIONS}"), |q| {
            q.bind(self.workspace)
                .bind(self.program)
                .bind(self.affiliate)
                .bind(self.status)
                .bind(self.from)
                .bind(self.to)
        })
        .await?;
        Ok(page.wrap((items, total)))
    }
}

fn conversion_cols() -> String {
    format!(
        "c.id, c.program_id, c.customer_id, c.affiliate_id, c.conversion_type_id, c.external_id, \
         c.amount_cents, c.commission_cents, c.currency::text AS currency, c.status::text AS status, \
         {} AS occurred_at, {} AS approved_at, {} AS reversed_at, c.payout_id",
        ts("c.occurred_at"),
        ts("c.approved_at"),
        ts("c.reversed_at")
    )
}

/// List conversions, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/conversions",
    tag = "conversions",
    security(("cookie" = [])),
    params(ConversionFilter, PageQuery),
    responses(
        (status = 200, body = Paged<Conversion>),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
    )
)]
pub async fn list_conversions(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<ConversionFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Conversion>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let sc = ConversionScope::new(
        (ws, f.affiliate_id),
        (f.program_id, f.status),
        (f.from.as_deref(), f.to.as_deref()),
    )?;
    Ok(Json(sc.list(&s, &conversion_cols(), &page).await?))
}

async fn get_view(s: &AppState, ws: Uuid, id: Uuid) -> Result<Conversion, AppError> {
    sqlx::query_as(&format!(
        "SELECT {} FROM {CONVERSIONS} AND c.id = $7",
        conversion_cols()
    ))
    .bind(ws)
    .bind(None::<Uuid>)
    .bind(None::<Uuid>)
    .bind(None::<&str>)
    .bind(None::<NaiveDate>)
    .bind(None::<NaiveDate>)
    .bind(id)
    .fetch_optional(&s.pool)
    .await?
    .ok_or(AppError::NotFound("conversion not found"))
}

/// One conversion.
#[utoipa::path(
    get,
    path = "/api/v1/conversions/{id}",
    tag = "conversions",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Conversion id")),
    responses((status = 200, body = Conversion), (status = 404, body = ErrorBody))
)]
pub async fn get_conversion(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversion>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    Ok(Json(get_view(&s, ws, id).await?))
}

/// Lock the conversion, run the state machine, audit, commit. An action the status does not
/// allow is `409` and changes nothing.
async fn run_action(
    s: &AppState,
    p: &Principal,
    id: Uuid,
    act: Act,
    name: &str,
) -> Result<Json<Conversion>, AppError> {
    let ws = p.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    let conv: Locked = sqlx::query_as(&format!(
        "SELECT {LOCKED_COLS} FROM conversions c JOIN programs p ON p.id = c.program_id \
         WHERE c.id = $1 AND p.workspace_id = $2 FOR UPDATE OF c"
    ))
    .bind(id)
    .bind(ws)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound("conversion not found"))?;
    let to = settle(&mut tx, &conv, act, "manual").await?;
    audit::record(
        &mut *tx,
        p,
        &format!("conversion.{name}"),
        "conversion",
        id,
        Some(json!({ "from": conv.status, "to": to })),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(get_view(s, ws, id).await?))
}

/// Approve a pending conversion: its commission moves from pending to payable.
#[utoipa::path(
    post,
    path = "/api/v1/conversions/{id}/approve",
    tag = "conversions",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Conversion id")),
    responses(
        (status = 200, body = Conversion),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Not pending", body = ErrorBody),
    )
)]
pub async fn approve(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversion>, AppError> {
    run_action(&s, &auth.principal, id, Act::Approve, "approve").await
}

/// Reject a pending conversion: its commission returns to expense.
#[utoipa::path(
    post,
    path = "/api/v1/conversions/{id}/reject",
    tag = "conversions",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Conversion id")),
    responses(
        (status = 200, body = Conversion),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Not pending", body = ErrorBody),
    )
)]
pub async fn reject(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversion>, AppError> {
    run_action(&s, &auth.principal, id, Act::Reject, "reject").await
}

/// Reverse an approved or paid conversion (refund, chargeback). After payout the affiliate's
/// payable goes negative and nets against their next batch.
#[utoipa::path(
    post,
    path = "/api/v1/conversions/{id}/reverse",
    tag = "conversions",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Conversion id")),
    responses(
        (status = 200, body = Conversion),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Not approved or paid", body = ErrorBody),
    )
)]
pub async fn reverse(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversion>, AppError> {
    run_action(&s, &auth.principal, id, Act::Reverse, "reverse").await
}

/// The signed-in affiliate's conversions, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/me/earnings",
    tag = "me",
    security(("cookie" = [])),
    params(EarningFilter, PageQuery),
    responses(
        (status = 200, body = Paged<Earning>),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
    )
)]
pub async fn me_earnings(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    Query(f): Query<EarningFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Earning>>, AppError> {
    let (ws, own) = viewer(&s, &auth.principal).await?;
    let own = own.ok_or(AppError::Forbidden("no affiliate profile"))?;
    let sc = ConversionScope::new(
        (ws, Some(own)),
        (f.program_id, f.status),
        (f.from.as_deref(), f.to.as_deref()),
    )?;
    let cols = format!(
        "c.id, c.program_id, c.amount_cents, c.commission_cents, c.currency::text AS currency, \
         c.status::text AS status, {} AS occurred_at, {} AS approved_at",
        ts("c.occurred_at"),
        ts("c.approved_at")
    );
    Ok(Json(sc.list(&s, &cols, &page).await?))
}

// ---- ledger

#[derive(Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Account {
    Expense,
    AffPending,
    AffPayable,
    Cash,
}

impl Account {
    fn as_str(self) -> &'static str {
        match self {
            Self::Expense => "expense",
            Self::AffPending => "aff_pending",
            Self::AffPayable => "aff_payable",
            Self::Cash => "cash",
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EntryFilter {
    program_id: Option<Uuid>,
    affiliate_id: Option<Uuid>,
    account: Option<Account>,
    /// All legs of one transaction.
    txn_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Entry {
    id: Uuid,
    /// Legs sharing a `txn_id` sum to zero.
    txn_id: Uuid,
    /// `expense`, `aff_pending`, `aff_payable` or `cash`.
    account: String,
    program_id: Uuid,
    affiliate_id: Option<Uuid>,
    /// Debit positive, credit negative.
    amount_cents: i64,
    currency: String,
    conversion_id: Option<Uuid>,
    payout_id: Option<Uuid>,
    created_at: String,
}

/// `$1` workspace, `$2` program, `$3` affiliate, `$4` account, `$5` txn.
const ENTRIES: &str = "ledger_entries e JOIN programs p ON p.id = e.program_id \
    WHERE p.workspace_id = $1 AND ($2::uuid IS NULL OR e.program_id = $2) \
      AND ($3::uuid IS NULL OR e.affiliate_id = $3) \
      AND ($4::text IS NULL OR e.account = $4::ledger_account) \
      AND ($5::uuid IS NULL OR e.txn_id = $5)";

/// Ledger legs, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/ledger/entries",
    tag = "ledger",
    security(("cookie" = [])),
    params(EntryFilter, PageQuery),
    responses((status = 200, body = Paged<Entry>), (status = 403, body = ErrorBody))
)]
pub async fn list_entries(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<EntryFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Entry>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let account = f.account.map(Account::as_str);
    let items = sqlx::query_as(&format!(
        "SELECT e.id, e.txn_id, e.account::text AS account, e.program_id, e.affiliate_id, \
                e.amount_cents, e.currency::text AS currency, e.conversion_id, e.payout_id, \
                {} AS created_at \
         FROM {ENTRIES} ORDER BY e.created_at DESC, e.id DESC LIMIT $6 OFFSET $7",
        ts("e.created_at")
    ))
    .bind(ws)
    .bind(f.program_id)
    .bind(f.affiliate_id)
    .bind(account)
    .bind(f.txn_id)
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(&s.pool)
    .await?;
    let total = count(&s, format!("SELECT count(*) FROM {ENTRIES}"), |q| {
        q.bind(ws)
            .bind(f.program_id)
            .bind(f.affiliate_id)
            .bind(account)
            .bind(f.txn_id)
    })
    .await?;
    Ok(Json(page.wrap((items, total))))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BalanceFilter {
    program_id: Option<Uuid>,
    /// Admins only: one affiliate. Affiliates always get their own balances.
    affiliate_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub struct Balance {
    program_id: Uuid,
    affiliate_id: Uuid,
    currency: String,
    /// Commission awaiting approval.
    pending_cents: i64,
    /// Approved and not yet paid out; negative after a clawback.
    payable_cents: i64,
}

/// `$1` workspace, `$2` program, `$3` affiliate.
const BALANCES: &str = "affiliate_balances b JOIN programs p ON p.id = b.program_id \
    WHERE p.workspace_id = $1 AND ($2::uuid IS NULL OR b.program_id = $2) \
      AND ($3::uuid IS NULL OR b.affiliate_id = $3)";

/// Pending and payable commission per affiliate and program, derived from the ledger.
#[utoipa::path(
    get,
    path = "/api/v1/ledger/balances",
    tag = "ledger",
    security(("cookie" = [])),
    params(BalanceFilter, PageQuery),
    responses(
        (status = 200, body = Paged<Balance>),
        (status = 404, description = "Unknown program or other affiliate", body = ErrorBody),
    )
)]
pub async fn list_balances(
    State(s): State<AppState>,
    auth: Auth<Any>,
    Query(f): Query<BalanceFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Balance>>, AppError> {
    let (ws, own) = viewer(&s, &auth.principal).await?;
    if own.is_some_and(|own| f.affiliate_id.is_some_and(|a| a != own)) {
        return Err(AppError::NotFound("affiliate not found"));
    }
    if let Some(id) = f.program_id {
        require_program(&mut *s.pool.acquire().await?, ws, id).await?;
    }
    let affiliate = own.or(f.affiliate_id);
    let items = sqlx::query_as(&format!(
        "SELECT b.program_id, b.affiliate_id, b.currency::text AS currency, b.pending_cents, \
                b.payable_cents \
         FROM {BALANCES} ORDER BY b.program_id, b.affiliate_id LIMIT $4 OFFSET $5"
    ))
    .bind(ws)
    .bind(f.program_id)
    .bind(affiliate)
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(&s.pool)
    .await?;
    let total = count(&s, format!("SELECT count(*) FROM {BALANCES}"), |q| {
        q.bind(ws).bind(f.program_id).bind(affiliate)
    })
    .await?;
    Ok(Json(page.wrap((items, total))))
}
