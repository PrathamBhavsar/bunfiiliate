//! `/api/v1/programs`: programs, secrets, sites, conversion types and commission plans.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bf_core::{Bps, CommissionKind, CommissionPlan, Currency, Duration};
use bf_db::auth::UserRole;
use bf_db::programs::{
    ConversionType, NewProgram, Plan, Program, ProgramPatch, Site, program_exists,
};
use bf_db::tracker::notify_config;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Any, Auth, Principal, random_hex};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::page::{PageQuery, Paged};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/programs", get(list_programs).post(create_program))
        .route(
            "/api/v1/programs/{id}",
            get(get_program)
                .patch(update_program)
                .delete(delete_program),
        )
        .route("/api/v1/programs/{id}/secrets/rotate", post(rotate_secrets))
        .route(
            "/api/v1/programs/{id}/sites",
            get(list_sites).post(add_site),
        )
        .route("/api/v1/programs/{id}/sites/{domain}", delete(delete_site))
        .route(
            "/api/v1/programs/{id}/conversion-types",
            get(list_conversion_types).post(create_conversion_type),
        )
        .route(
            "/api/v1/programs/{id}/conversion-types/{type_id}",
            delete(delete_conversion_type),
        )
        .route(
            "/api/v1/programs/{id}/plans",
            get(list_plans).post(create_plan),
        )
        .route("/api/v1/programs/{id}/plans/{plan_id}", delete(delete_plan))
}

fn not_found() -> AppError {
    AppError::NotFound("program not found")
}

pub(crate) async fn require_program(
    db: &mut PgConnection,
    ws: Uuid,
    id: Uuid,
) -> Result<(), AppError> {
    if program_exists(db, ws, id).await? {
        Ok(())
    } else {
        Err(not_found())
    }
}

/// The workspace to read in, and for affiliates the affiliate whose enrollments limit the view.
pub(crate) async fn viewer(s: &AppState, p: &Principal) -> Result<(Uuid, Option<Uuid>), AppError> {
    let ws = p.workspace_scope(None)?;
    if p.role != UserRole::Affiliate {
        return Ok((ws, None));
    }
    let aff = bf_db::affiliates::affiliate_for_user(&s.pool, ws, p.user_id)
        .await?
        .ok_or(AppError::Forbidden("no affiliate profile"))?;
    Ok((ws, Some(aff.id)))
}

// ---- programs

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttributionModel {
    FirstTouch,
    LastTouch,
}

impl AttributionModel {
    fn as_str(&self) -> &'static str {
        match self {
            Self::FirstTouch => "first_touch",
            Self::LastTouch => "last_touch",
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct NewProgramRequest {
    name: String,
    /// ISO 4217 code, e.g. `USD`.
    currency: String,
    /// Default `last_touch`.
    attribution_model: Option<AttributionModel>,
    /// Default 30.
    cookie_days: Option<i32>,
    /// Default false.
    auto_approve: Option<bool>,
    /// Default 0.
    hold_days: Option<i32>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct ProgramUpdate {
    name: Option<String>,
    attribution_model: Option<AttributionModel>,
    cookie_days: Option<i32>,
    auto_approve: Option<bool>,
    hold_days: Option<i32>,
    /// Must be a plan of this program.
    default_plan_id: Option<Uuid>,
    /// Empty string clears it.
    #[serde(skip_serializing)]
    stripe_secret: Option<String>,
    /// Empty string clears it.
    #[serde(skip_serializing)]
    paddle_secret: Option<String>,
}

fn check_name(name: &str) -> Result<&str, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    Ok(name)
}

fn check_program_numbers(cookie_days: Option<i32>, hold_days: Option<i32>) -> Result<(), AppError> {
    if cookie_days.is_some_and(|d| d < 1) {
        return Err(AppError::BadRequest(
            "cookie_days must be at least 1".into(),
        ));
    }
    if hold_days.is_some_and(|d| d < 0) {
        return Err(AppError::BadRequest(
            "hold_days must not be negative".into(),
        ));
    }
    if cookie_days.is_some_and(|d| d > MAX_DAYS) || hold_days.is_some_and(|d| d > MAX_DAYS) {
        return Err(AppError::BadRequest(format!(
            "cookie_days and hold_days must be at most {MAX_DAYS}"
        )));
    }
    Ok(())
}

/// Longest cookie window or hold period, in days (also a database CHECK).
const MAX_DAYS: i32 = 365;

/// Currencies a program may pay in: ISO 4217 codes with two decimal places, because payouts
/// and the CSV export count in hundredths. Zero-decimal (JPY) and three-decimal (KWD)
/// currencies would be exported off by 100x and 10x.
const CURRENCIES: [&str; 22] = [
    "USD", "EUR", "GBP", "CAD", "AUD", "NZD", "CHF", "SEK", "NOK", "DKK", "PLN", "CZK", "SGD",
    "HKD", "MXN", "BRL", "ZAR", "INR", "AED", "ILS", "TRY", "THB",
];

/// Create a program with fresh public key and webhook secret.
#[utoipa::path(
    post,
    path = "/api/v1/programs",
    tag = "programs",
    security(("cookie" = [])),
    request_body = NewProgramRequest,
    responses((status = 201, body = Program), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody))
)]
pub async fn create_program(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<NewProgramRequest>,
) -> Result<(StatusCode, Json<Program>), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    crate::billing::enforce(&s.pool, ws, bf_core::billing::Limit::Programs).await?;
    let name = check_name(&req.name)?;
    let currency = Currency::new(&req.currency.to_ascii_uppercase())
        .filter(|c| CURRENCIES.contains(&c.as_str()))
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "currency must be one of the supported two-decimal codes: {}",
                CURRENCIES.join(", ")
            ))
        })?;
    check_program_numbers(req.cookie_days, req.hold_days)?;
    let (public_key, webhook_secret) = (
        format!("pk_{}", random_hex(16)),
        format!("whsec_{}", random_hex(32)),
    );
    let id = Uuid::now_v7();
    let sealed_secret = s
        .vault
        .seal_col("programs", "webhook_secret", id, &webhook_secret);
    let new = NewProgram {
        id,
        workspace_id: ws,
        name,
        currency: currency.as_str(),
        attribution_model: req
            .attribution_model
            .as_ref()
            .map_or("last_touch", |m| m.as_str()),
        cookie_days: req.cookie_days.unwrap_or(30),
        auto_approve: req.auto_approve.unwrap_or(false),
        hold_days: req.hold_days.unwrap_or(0),
        public_key: &public_key,
        webhook_secret: &sealed_secret,
    };
    let mut tx = s.pool.begin().await?;
    let program = bf_db::programs::insert_program(&mut *tx, &new).await?;
    let diff = json!({ "name": program.name, "currency": program.currency });
    audit::record(
        &mut *tx,
        &auth.principal,
        "program.create",
        "program",
        program.id,
        Some(diff),
    )
    .await?;
    notify_config(&mut *tx).await?;
    tx.commit().await?;
    // The only time the secret is shown: the response carries the plaintext, never the stored value.
    let mut program = program.redact();
    program.webhook_secret = Some(webhook_secret);
    Ok((StatusCode::CREATED, Json(program)))
}

/// Programs of the workspace (affiliates: only the ones they are enrolled in, without secrets).
#[utoipa::path(
    get,
    path = "/api/v1/programs",
    tag = "programs",
    security(("cookie" = [])),
    params(PageQuery),
    responses((status = 200, body = Paged<Program>), (status = 401, body = ErrorBody))
)]
pub async fn list_programs(
    State(s): State<AppState>,
    auth: Auth<Any>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Program>>, AppError> {
    let (ws, only_for) = viewer(&s, &auth.principal).await?;
    let (items, total) =
        bf_db::programs::list_programs(&s.pool, ws, only_for, page.limit(), page.offset()).await?;
    let items = items.into_iter().map(Program::redact).collect();
    Ok(Json(page.wrap((items, total))))
}

/// One program. Secrets are never returned; they are shown once, on create and rotate.
#[utoipa::path(
    get,
    path = "/api/v1/programs/{id}",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id")),
    responses((status = 200, body = Program), (status = 404, body = ErrorBody))
)]
pub async fn get_program(
    State(s): State<AppState>,
    auth: Auth<Any>,
    Path(id): Path<Uuid>,
) -> Result<Json<Program>, AppError> {
    let (ws, only_for) = viewer(&s, &auth.principal).await?;
    let program = bf_db::programs::get_program(&s.pool, ws, id, only_for)
        .await?
        .ok_or_else(not_found)?;
    Ok(Json(program.redact()))
}

/// Update program settings. Provider secrets can be set here; the webhook secret is rotated.
#[utoipa::path(
    patch,
    path = "/api/v1/programs/{id}",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id")),
    request_body = ProgramUpdate,
    responses((status = 200, body = Program), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn update_program(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<ProgramUpdate>,
) -> Result<Json<Program>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let name = req.name.as_deref().map(check_name).transpose()?;
    check_program_numbers(req.cookie_days, req.hold_days)?;
    let mut tx = s.pool.begin().await?;
    if let Some(plan) = req.default_plan_id
        && !bf_db::programs::plan_in_program(&mut *tx, id, plan).await?
    {
        return Err(AppError::BadRequest(
            "default_plan_id is not a plan of this program".into(),
        ));
    }
    let stripe = req
        .stripe_secret
        .as_deref()
        .map(|v| seal_optional(&s, id, "stripe_secret", v));
    let paddle = req
        .paddle_secret
        .as_deref()
        .map(|v| seal_optional(&s, id, "paddle_secret", v));
    let patch = ProgramPatch {
        name,
        attribution_model: req.attribution_model.as_ref().map(|m| m.as_str()),
        cookie_days: req.cookie_days,
        auto_approve: req.auto_approve,
        hold_days: req.hold_days,
        default_plan_id: req.default_plan_id,
        stripe_secret: stripe.as_deref(),
        paddle_secret: paddle.as_deref(),
    };
    let program = bf_db::programs::update_program(&mut *tx, ws, id, &patch)
        .await?
        .ok_or_else(not_found)?;
    let mut diff = json!(req);
    diff["secrets_changed"] = json!(req.stripe_secret.is_some() || req.paddle_secret.is_some());
    audit::record(
        &mut *tx,
        &auth.principal,
        "program.update",
        "program",
        id,
        Some(diff),
    )
    .await?;
    notify_config(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(program.redact()))
}

/// Seal a provider secret for storage; the empty string means "clear" and stays empty.
fn seal_optional(s: &AppState, id: Uuid, column: &str, plain: &str) -> String {
    if plain.is_empty() {
        String::new()
    } else {
        s.vault.seal_col("programs", column, id, plain)
    }
}

/// Delete a program and its sites, types, plans, enrollments and codes. 409 once it has clicks or conversions.
#[utoipa::path(
    delete,
    path = "/api/v1/programs/{id}",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id")),
    responses((status = 204), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn delete_program(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    if !bf_db::programs::delete_program(&mut *tx, ws, id).await? {
        return Err(not_found());
    }
    audit::record(
        &mut *tx,
        &auth.principal,
        "program.delete",
        "program",
        id,
        None,
    )
    .await?;
    notify_config(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Generate a new webhook secret. The old one stops verifying once the tracker reloads.
#[utoipa::path(
    post,
    path = "/api/v1/programs/{id}/secrets/rotate",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id")),
    responses((status = 200, body = Program), (status = 404, body = ErrorBody))
)]
pub async fn rotate_secrets(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Program>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let secret = format!("whsec_{}", random_hex(32));
    let mut tx = s.pool.begin().await?;
    let sealed = s.vault.seal_col("programs", "webhook_secret", id, &secret);
    let program = bf_db::programs::set_webhook_secret(&mut *tx, ws, id, &sealed)
        .await?
        .ok_or_else(not_found)?;
    audit::record(
        &mut *tx,
        &auth.principal,
        "program.rotate_secret",
        "program",
        id,
        None,
    )
    .await?;
    notify_config(&mut *tx).await?;
    tx.commit().await?;
    let mut program = program.redact();
    program.webhook_secret = Some(secret);
    Ok(Json(program))
}

// ---- sites

#[derive(Deserialize, ToSchema)]
pub struct NewSite {
    /// Bare host, e.g. `shop.example.com`.
    domain: String,
}

fn normalize_domain(d: &str) -> Result<String, AppError> {
    let d = d.trim().to_ascii_lowercase();
    let ok = !d.is_empty()
        && d.len() <= 253
        && d.split('.').all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    if ok {
        Ok(d)
    } else {
        Err(AppError::BadRequest(
            "domain must be a bare host name".into(),
        ))
    }
}

/// Add a site domain.
#[utoipa::path(
    post,
    path = "/api/v1/programs/{id}/sites",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id")),
    request_body = NewSite,
    responses((status = 201, body = Site), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn add_site(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<NewSite>,
) -> Result<(StatusCode, Json<Site>), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let domain = normalize_domain(&req.domain)?;
    let mut tx = s.pool.begin().await?;
    if !bf_db::programs::insert_site(&mut *tx, ws, id, &domain).await? {
        return Err(not_found());
    }
    let diff = json!({ "domain": domain });
    audit::record(
        &mut *tx,
        &auth.principal,
        "site.add",
        "program",
        id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(Site { domain })))
}

/// List the program's site domains.
#[utoipa::path(
    get,
    path = "/api/v1/programs/{id}/sites",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id"), PageQuery),
    responses((status = 200, body = Paged<Site>), (status = 404, body = ErrorBody))
)]
pub async fn list_sites(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Site>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    require_program(&mut *s.pool.acquire().await?, ws, id).await?;
    let rows = bf_db::programs::list_sites(&s.pool, ws, id, page.limit(), page.offset()).await?;
    Ok(Json(page.wrap(rows)))
}

/// Remove a site domain.
#[utoipa::path(
    delete,
    path = "/api/v1/programs/{id}/sites/{domain}",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id"), ("domain" = String, Path, description = "Site domain")),
    responses((status = 204), (status = 404, body = ErrorBody))
)]
pub async fn delete_site(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path((id, domain)): Path<(Uuid, String)>,
) -> Result<StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let domain = domain.to_ascii_lowercase();
    let mut tx = s.pool.begin().await?;
    if !bf_db::programs::delete_site(&mut *tx, ws, id, &domain).await? {
        return Err(AppError::NotFound("site not found"));
    }
    if bf_db::programs::delete_offsite_codes(&mut *tx, ws, id).await? > 0 {
        notify_config(&mut *tx).await?;
    }
    let diff = json!({ "domain": domain });
    audit::record(
        &mut *tx,
        &auth.principal,
        "site.delete",
        "program",
        id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- conversion types

#[derive(Deserialize, Serialize, ToSchema)]
pub struct NewConversionType {
    /// Stable identifier sent by events, e.g. `signup`.
    key: String,
    name: String,
}

/// Add a conversion type.
#[utoipa::path(
    post,
    path = "/api/v1/programs/{id}/conversion-types",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id")),
    request_body = NewConversionType,
    responses((status = 201, body = ConversionType), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn create_conversion_type(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<NewConversionType>,
) -> Result<(StatusCode, Json<ConversionType>), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let (key, name) = (check_name(&req.key)?, check_name(&req.name)?);
    let t = ConversionType {
        id: Uuid::now_v7(),
        program_id: id,
        key: key.to_string(),
        name: name.to_string(),
    };
    let mut tx = s.pool.begin().await?;
    if !bf_db::programs::insert_conversion_type(&mut *tx, ws, &t).await? {
        return Err(not_found());
    }
    audit::record(
        &mut *tx,
        &auth.principal,
        "conversion_type.create",
        "conversion_type",
        t.id,
        Some(json!(req)),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(t)))
}

/// List conversion types.
#[utoipa::path(
    get,
    path = "/api/v1/programs/{id}/conversion-types",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id"), PageQuery),
    responses((status = 200, body = Paged<ConversionType>), (status = 404, body = ErrorBody))
)]
pub async fn list_conversion_types(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<ConversionType>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    require_program(&mut *s.pool.acquire().await?, ws, id).await?;
    let rows = bf_db::programs::list_conversion_types(&s.pool, ws, id, page.limit(), page.offset())
        .await?;
    Ok(Json(page.wrap(rows)))
}

/// Delete a conversion type. 409 while a plan uses it.
#[utoipa::path(
    delete,
    path = "/api/v1/programs/{id}/conversion-types/{type_id}",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id"), ("type_id" = Uuid, Path, description = "Conversion type id")),
    responses((status = 204), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn delete_conversion_type(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path((id, type_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    if !bf_db::programs::delete_conversion_type(&mut *tx, ws, id, type_id).await? {
        return Err(AppError::NotFound("conversion type not found"));
    }
    audit::record(
        &mut *tx,
        &auth.principal,
        "conversion_type.delete",
        "conversion_type",
        type_id,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- commission plans

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    Percent,
    Flat,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanDuration {
    OneMonth,
    XMonths,
    Lifetime,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct NewPlan {
    /// Plan for one conversion type only; omit for a plan that applies to all.
    conversion_type_id: Option<Uuid>,
    kind: PlanKind,
    /// `percent` plans: 0..=10000 (100%).
    rate_bps: Option<u32>,
    /// `flat` plans: cents per conversion.
    amount_cents: Option<i64>,
    duration: PlanDuration,
    /// `x_months` plans: number of months, at least 1.
    months: Option<u32>,
}

impl NewPlan {
    /// Build the bf-core plan (its range checks are the validation) and the row to store.
    fn validate(&self, program_id: Uuid) -> Result<Plan, AppError> {
        let bad = |m: &str| AppError::BadRequest(m.into());
        let kind = match (&self.kind, self.rate_bps, self.amount_cents) {
            (PlanKind::Percent, Some(bps), None) => CommissionKind::Percent(
                Bps::new(bps).ok_or_else(|| bad("rate_bps must be 0..=10000"))?,
            ),
            (PlanKind::Flat, None, Some(c)) if c >= 0 => CommissionKind::Flat(c),
            (PlanKind::Flat, None, Some(_)) => {
                return Err(bad("amount_cents must not be negative"));
            }
            (PlanKind::Percent, ..) => return Err(bad("percent plan takes rate_bps only")),
            (PlanKind::Flat, ..) => return Err(bad("flat plan takes amount_cents only")),
        };
        let (duration, months) = match (&self.duration, self.months) {
            (PlanDuration::XMonths, Some(m)) if m >= 1 => (Duration::XMonths(m), Some(m)),
            (PlanDuration::XMonths, _) => return Err(bad("x_months plan needs months >= 1")),
            (_, Some(_)) => return Err(bad("months only applies to x_months plans")),
            (PlanDuration::OneMonth, None) => (Duration::OneMonth, None),
            (PlanDuration::Lifetime, None) => (Duration::Lifetime, None),
        };
        let plan = CommissionPlan { kind, duration };
        let to_i32 = |v: u32| i32::try_from(v).map_err(|_| bad("months too large"));
        Ok(Plan {
            id: Uuid::now_v7(),
            program_id,
            conversion_type_id: self.conversion_type_id,
            kind: match plan.kind {
                CommissionKind::Percent(_) => "percent",
                CommissionKind::Flat(_) => "flat",
            }
            .into(),
            rate_bps: match plan.kind {
                CommissionKind::Percent(b) => Some(to_i32(b.get())?),
                CommissionKind::Flat(_) => None,
            },
            amount_cents: match plan.kind {
                CommissionKind::Flat(c) => Some(c),
                CommissionKind::Percent(_) => None,
            },
            duration: match plan.duration {
                Duration::OneMonth => "one_month",
                Duration::XMonths(_) => "x_months",
                Duration::Lifetime => "lifetime",
            }
            .into(),
            months: months.map(to_i32).transpose()?,
        })
    }
}

/// Add a commission plan.
#[utoipa::path(
    post,
    path = "/api/v1/programs/{id}/plans",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id")),
    request_body = NewPlan,
    responses((status = 201, body = Plan), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn create_plan(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<NewPlan>,
) -> Result<(StatusCode, Json<Plan>), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let plan = req.validate(id)?;
    let mut tx = s.pool.begin().await?;
    if let Some(t) = plan.conversion_type_id
        && !bf_db::programs::conversion_type_in_program(&mut *tx, id, t).await?
    {
        return Err(AppError::BadRequest(
            "conversion_type_id is not a type of this program".into(),
        ));
    }
    if !bf_db::programs::insert_plan(&mut *tx, ws, &plan).await? {
        return Err(not_found());
    }
    audit::record(
        &mut *tx,
        &auth.principal,
        "plan.create",
        "plan",
        plan.id,
        Some(json!(req)),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(plan)))
}

/// List commission plans.
#[utoipa::path(
    get,
    path = "/api/v1/programs/{id}/plans",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id"), PageQuery),
    responses((status = 200, body = Paged<Plan>), (status = 404, body = ErrorBody))
)]
pub async fn list_plans(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Plan>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    require_program(&mut *s.pool.acquire().await?, ws, id).await?;
    let rows = bf_db::programs::list_plans(&s.pool, ws, id, page.limit(), page.offset()).await?;
    Ok(Json(page.wrap(rows)))
}

/// Delete a plan. 409 while it is the program default or an enrollment override.
#[utoipa::path(
    delete,
    path = "/api/v1/programs/{id}/plans/{plan_id}",
    tag = "programs",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id"), ("plan_id" = Uuid, Path, description = "Plan id")),
    responses((status = 204), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn delete_plan(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path((id, plan_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    if !bf_db::programs::delete_plan(&mut *tx, ws, id, plan_id).await? {
        return Err(AppError::NotFound("plan not found"));
    }
    audit::record(
        &mut *tx,
        &auth.principal,
        "plan.delete",
        "plan",
        plan_id,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
