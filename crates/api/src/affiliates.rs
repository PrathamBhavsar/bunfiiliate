//! `/api/v1/affiliates`, enrollments, referral codes and the affiliate's own `/me/*` views.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bf_db::affiliates::{Affiliate, Enrollment, ReferralCode};
use bf_db::auth::{User, UserRole};
use bf_db::tracker::notify_config;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Affiliate as AffiliateRole, Auth, Principal, hash_password, random_hex};
use crate::bootstrap::MIN_PASSWORD_LEN;
use crate::error::{AppError, AppJson, ErrorBody};
use crate::page::{PageQuery, Paged};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/affiliates", get(list_affiliates))
        .route("/api/v1/affiliates/register", post(register))
        .route(
            "/api/v1/affiliates/{id}",
            get(get_affiliate).patch(update_affiliate),
        )
        .route("/api/v1/affiliates/{id}/approve", post(approve))
        .route("/api/v1/affiliates/{id}/suspend", post(suspend))
        .route("/api/v1/affiliates/{id}/reactivate", post(reactivate))
        .route(
            "/api/v1/programs/{id}/enrollments",
            get(list_enrollments).post(enroll),
        )
        .route(
            "/api/v1/programs/{id}/enrollments/{affiliate_id}",
            delete(unenroll),
        )
        .route("/api/v1/referral-codes", get(list_codes).post(create_code))
        .route("/api/v1/referral-codes/{code}", delete(delete_code))
        .route("/api/v1/me/affiliate", get(me_affiliate).patch(me_update))
        .route(
            "/api/v1/me/referral-codes",
            get(me_list_codes).post(me_create_code),
        )
        .route("/api/v1/me/referral-codes/{code}", delete(me_delete_code))
}

fn email_ok(e: &str) -> bool {
    e.contains('@')
}

async fn own_affiliate(s: &AppState, p: &Principal) -> Result<Affiliate, AppError> {
    let ws = p.workspace_scope(None)?;
    bf_db::affiliates::affiliate_for_user(&s.pool, ws, p.user_id)
        .await?
        .ok_or(AppError::Forbidden("no affiliate profile"))
}

// ---- registration and review

#[derive(Deserialize, ToSchema)]
pub struct Register {
    /// Slug of the workspace to join.
    workspace: String,
    email: String,
    /// Min 8 characters.
    password: String,
    payout_email: Option<String>,
}

/// Public sign-up: creates a user and a `pending` affiliate awaiting approval.
#[utoipa::path(
    post,
    path = "/api/v1/affiliates/register",
    tag = "affiliates",
    request_body = Register,
    responses(
        (status = 201, body = Affiliate),
        (status = 400, body = ErrorBody),
        (status = 404, description = "Unknown workspace", body = ErrorBody),
        (status = 409, description = "Email taken", body = ErrorBody),
    )
)]
pub async fn register(
    State(s): State<AppState>,
    AppJson(req): AppJson<Register>,
) -> Result<(StatusCode, Json<Affiliate>), AppError> {
    let email = req.email.trim();
    if !email_ok(email) || req.payout_email.as_deref().is_some_and(|e| !email_ok(e)) {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    if req.password.len() < MIN_PASSWORD_LEN {
        return Err(AppError::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let ws = bf_db::workspaces::workspace_id_by_slug(&s.pool, &req.workspace)
        .await?
        .ok_or(AppError::NotFound("workspace not found"))?;
    let password = req.password;
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| AppError::Internal)??;
    let user = User {
        id: Uuid::now_v7(),
        workspace_id: Some(ws),
        email: email.to_string(),
        password_hash,
        role: UserRole::Affiliate,
    };
    let id = Uuid::now_v7();
    let mut tx = s.pool.begin().await?;
    bf_db::auth::create_user(&mut *tx, &user).await?;
    bf_db::affiliates::insert_affiliate(&mut *tx, id, ws, user.id, req.payout_email.as_deref())
        .await?;
    let diff = json!({ "email": user.email });
    audit::record(
        &mut *tx,
        &Principal::from(&user),
        "affiliate.register",
        "affiliate",
        id,
        Some(diff),
    )
    .await?;
    let affiliate = bf_db::affiliates::get_affiliate(&mut *tx, ws, id)
        .await?
        .ok_or(AppError::Internal)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(affiliate)))
}

#[derive(Deserialize, IntoParams)]
pub struct StatusFilter {
    /// `pending`, `active` or `suspended`.
    status: Option<String>,
}

/// List affiliates, optionally by status.
#[utoipa::path(
    get,
    path = "/api/v1/affiliates",
    tag = "affiliates",
    security(("cookie" = [])),
    params(StatusFilter, PageQuery),
    responses((status = 200, body = Paged<Affiliate>), (status = 403, body = ErrorBody))
)]
pub async fn list_affiliates(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<StatusFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Affiliate>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rows = bf_db::affiliates::list_affiliates(
        &s.pool,
        ws,
        f.status.as_deref(),
        page.limit(),
        page.offset(),
    )
    .await?;
    Ok(Json(page.wrap(rows)))
}

/// One affiliate.
#[utoipa::path(
    get,
    path = "/api/v1/affiliates/{id}",
    tag = "affiliates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Affiliate id")),
    responses((status = 200, body = Affiliate), (status = 404, body = ErrorBody))
)]
pub async fn get_affiliate(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Affiliate>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let a = bf_db::affiliates::get_affiliate(&s.pool, ws, id)
        .await?
        .ok_or(AppError::NotFound("affiliate not found"))?;
    Ok(Json(a))
}

async fn transition(
    s: &AppState,
    p: &Principal,
    id: Uuid,
    from: &[&str],
    to: &str,
) -> Result<Json<Affiliate>, AppError> {
    let ws = p.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    if !bf_db::affiliates::set_affiliate_status(&mut *tx, ws, id, from, to).await? {
        let exists = bf_db::affiliates::get_affiliate(&mut *tx, ws, id)
            .await?
            .is_some();
        return Err(if exists {
            AppError::Conflict("status change not allowed from current status")
        } else {
            AppError::NotFound("affiliate not found")
        });
    }
    if to == "suspended"
        && bf_db::affiliates::delete_codes_of_affiliate(&mut *tx, ws, id).await? > 0
    {
        notify_config(&mut *tx).await?;
    }
    let diff = json!({ "status": to });
    audit::record(
        &mut *tx,
        p,
        &format!("affiliate.{to}"),
        "affiliate",
        id,
        Some(diff),
    )
    .await?;
    let a = bf_db::affiliates::get_affiliate(&mut *tx, ws, id)
        .await?
        .ok_or(AppError::Internal)?;
    tx.commit().await?;
    Ok(Json(a))
}

/// Approve a pending affiliate.
#[utoipa::path(
    post,
    path = "/api/v1/affiliates/{id}/approve",
    tag = "affiliates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Affiliate id")),
    responses((status = 200, body = Affiliate), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn approve(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Affiliate>, AppError> {
    transition(&s, &auth.principal, id, &["pending"], "active").await
}

/// Suspend a pending or active affiliate.
#[utoipa::path(
    post,
    path = "/api/v1/affiliates/{id}/suspend",
    tag = "affiliates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Affiliate id")),
    responses((status = 200, body = Affiliate), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn suspend(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Affiliate>, AppError> {
    transition(&s, &auth.principal, id, &["pending", "active"], "suspended").await
}

/// Reactivate a suspended affiliate.
#[utoipa::path(
    post,
    path = "/api/v1/affiliates/{id}/reactivate",
    tag = "affiliates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Affiliate id")),
    responses((status = 200, body = Affiliate), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
pub async fn reactivate(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<Affiliate>, AppError> {
    transition(&s, &auth.principal, id, &["suspended"], "active").await
}

// ---- payout settings

#[derive(Deserialize, Serialize, ToSchema)]
pub struct PayoutUpdate {
    payout_email: Option<String>,
    /// Minimum balance in cents before a payout is created.
    payout_min_cents: Option<i64>,
}

async fn update_payout(
    s: &AppState,
    p: &Principal,
    id: Uuid,
    req: PayoutUpdate,
) -> Result<Json<Affiliate>, AppError> {
    let ws = p.workspace_scope(None)?;
    if req.payout_email.as_deref().is_some_and(|e| !email_ok(e)) {
        return Err(AppError::BadRequest("invalid payout_email".into()));
    }
    if req.payout_min_cents.is_some_and(|c| c < 0) {
        return Err(AppError::BadRequest(
            "payout_min_cents must not be negative".into(),
        ));
    }
    let mut tx = s.pool.begin().await?;
    let found = bf_db::affiliates::update_payout(
        &mut *tx,
        ws,
        id,
        req.payout_email.as_deref(),
        req.payout_min_cents,
    )
    .await?;
    if !found {
        return Err(AppError::NotFound("affiliate not found"));
    }
    audit::record(
        &mut *tx,
        p,
        "affiliate.update",
        "affiliate",
        id,
        Some(json!(req)),
    )
    .await?;
    let a = bf_db::affiliates::get_affiliate(&mut *tx, ws, id)
        .await?
        .ok_or(AppError::Internal)?;
    tx.commit().await?;
    Ok(Json(a))
}

/// Change an affiliate's payout email or minimum.
#[utoipa::path(
    patch,
    path = "/api/v1/affiliates/{id}",
    tag = "affiliates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Affiliate id")),
    request_body = PayoutUpdate,
    responses((status = 200, body = Affiliate), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
pub async fn update_affiliate(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<PayoutUpdate>,
) -> Result<Json<Affiliate>, AppError> {
    update_payout(&s, &auth.principal, id, req).await
}

/// The signed-in affiliate's profile.
#[utoipa::path(
    get,
    path = "/api/v1/me/affiliate",
    tag = "me",
    security(("cookie" = [])),
    responses((status = 200, body = Affiliate), (status = 403, body = ErrorBody))
)]
pub async fn me_affiliate(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
) -> Result<Json<Affiliate>, AppError> {
    Ok(Json(own_affiliate(&s, &auth.principal).await?))
}

/// Change the signed-in affiliate's payout email or minimum.
#[utoipa::path(
    patch,
    path = "/api/v1/me/affiliate",
    tag = "me",
    security(("cookie" = [])),
    request_body = PayoutUpdate,
    responses((status = 200, body = Affiliate), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody))
)]
pub async fn me_update(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    AppJson(req): AppJson<PayoutUpdate>,
) -> Result<Json<Affiliate>, AppError> {
    let own = own_affiliate(&s, &auth.principal).await?;
    update_payout(&s, &auth.principal, own.id, req).await
}

// ---- enrollments

#[derive(Deserialize, Serialize, ToSchema)]
pub struct NewEnrollment {
    affiliate_id: Uuid,
    /// Plan of this program that replaces the default for this affiliate.
    plan_override_id: Option<Uuid>,
}

/// Enroll an affiliate, or change the plan override of an existing enrollment.
#[utoipa::path(
    post,
    path = "/api/v1/programs/{id}/enrollments",
    tag = "affiliates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id")),
    request_body = NewEnrollment,
    responses(
        (status = 201, body = Enrollment),
        (status = 404, description = "Program, affiliate or plan not found", body = ErrorBody),
    )
)]
pub async fn enroll(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<NewEnrollment>,
) -> Result<(StatusCode, Json<Enrollment>), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    let e = bf_db::affiliates::upsert_enrollment(
        &mut *tx,
        ws,
        id,
        req.affiliate_id,
        req.plan_override_id,
    )
    .await?
    .ok_or(AppError::NotFound("program, affiliate or plan not found"))?;
    audit::record(
        &mut *tx,
        &auth.principal,
        "enrollment.set",
        "program",
        id,
        Some(json!(req)),
    )
    .await?;
    notify_config(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(e)))
}

/// List a program's enrollments.
#[utoipa::path(
    get,
    path = "/api/v1/programs/{id}/enrollments",
    tag = "affiliates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id"), PageQuery),
    responses((status = 200, body = Paged<Enrollment>), (status = 404, body = ErrorBody))
)]
pub async fn list_enrollments(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<Enrollment>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    if !bf_db::programs::program_exists(&s.pool, ws, id).await? {
        return Err(AppError::NotFound("program not found"));
    }
    let rows =
        bf_db::affiliates::list_enrollments(&s.pool, ws, id, page.limit(), page.offset()).await?;
    Ok(Json(page.wrap(rows)))
}

/// Remove an enrollment and the affiliate's referral codes for the program.
#[utoipa::path(
    delete,
    path = "/api/v1/programs/{id}/enrollments/{affiliate_id}",
    tag = "affiliates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Program id"), ("affiliate_id" = Uuid, Path, description = "Affiliate id")),
    responses((status = 204), (status = 404, body = ErrorBody))
)]
pub async fn unenroll(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path((id, affiliate_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    if !bf_db::affiliates::delete_enrollment(&mut tx, ws, id, affiliate_id).await? {
        return Err(AppError::NotFound("enrollment not found"));
    }
    let diff = json!({ "affiliate_id": affiliate_id });
    audit::record(
        &mut *tx,
        &auth.principal,
        "enrollment.delete",
        "program",
        id,
        Some(diff),
    )
    .await?;
    notify_config(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- referral codes

const CODE_MIN: usize = 3;
const CODE_MAX: usize = 32;

/// Host of an http(s) URL, or `None` for anything that could smuggle another host
/// (userinfo, backslashes, whitespace, control characters).
fn landing_host(url: &str) -> Option<&str> {
    if url
        .bytes()
        .any(|b| b.is_ascii_control() || b == b' ' || b == b'\\')
    {
        return None;
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

fn on_sites(host: &str, sites: &[String]) -> bool {
    let host = host.to_ascii_lowercase();
    sites
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

struct CodeSpec<'a> {
    affiliate_id: Uuid,
    program_id: Uuid,
    code: Option<&'a str>,
    landing_url: Option<&'a str>,
}

/// Create a code and tell the tracker. Landing pages must sit on one of the program's
/// sites so `/r/:code` can't be turned into an open redirect.
async fn insert_code(
    s: &AppState,
    p: &Principal,
    ws: Uuid,
    spec: CodeSpec<'_>,
) -> Result<(StatusCode, Json<ReferralCode>), AppError> {
    let bad = |m: &str| AppError::BadRequest(m.into());
    let code = match spec.code {
        Some(c)
            if (CODE_MIN..=CODE_MAX).contains(&c.len())
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
        {
            c.to_string()
        }
        Some(_) => return Err(bad("code must be 3-32 characters of A-Z a-z 0-9 - _")),
        None => random_hex(5),
    };
    let mut tx = s.pool.begin().await?;
    if !bf_db::programs::program_exists(&mut *tx, ws, spec.program_id).await? {
        return Err(AppError::NotFound("program not found"));
    }
    let sites = bf_db::programs::site_domains(&mut *tx, ws, spec.program_id).await?;
    let landing_url = match spec.landing_url {
        Some(u) => u.to_string(),
        None => format!(
            "https://{}",
            sites.first().ok_or_else(|| bad("program has no sites"))?
        ),
    };
    if !landing_host(&landing_url).is_some_and(|h| on_sites(h, &sites)) {
        return Err(bad(
            "landing_url must be an http(s) URL on one of the program's sites",
        ));
    }
    let c = ReferralCode {
        code,
        affiliate_id: spec.affiliate_id,
        program_id: spec.program_id,
        landing_url,
    };
    if !bf_db::affiliates::insert_code(&mut *tx, ws, &c).await? {
        return Err(AppError::Conflict(
            "affiliate must be active and enrolled in the program",
        ));
    }
    let diff = json!({ "affiliate_id": c.affiliate_id, "program_id": c.program_id });
    audit::record(
        &mut *tx,
        p,
        "code.create",
        "referral_code",
        &c.code,
        Some(diff),
    )
    .await?;
    notify_config(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(c)))
}

async fn remove_code(
    s: &AppState,
    p: &Principal,
    ws: Uuid,
    code: &str,
    affiliate_id: Option<Uuid>,
) -> Result<StatusCode, AppError> {
    let mut tx = s.pool.begin().await?;
    if !bf_db::affiliates::delete_code(&mut *tx, ws, code, affiliate_id).await? {
        return Err(AppError::NotFound("code not found"));
    }
    audit::record(&mut *tx, p, "code.deactivate", "referral_code", code, None).await?;
    notify_config(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
pub struct NewCode {
    affiliate_id: Uuid,
    program_id: Uuid,
    /// Generated when omitted.
    code: Option<String>,
    /// Defaults to the program's first site.
    landing_url: Option<String>,
}

/// Create a referral code for an active, enrolled affiliate. Notifies the tracker.
#[utoipa::path(
    post,
    path = "/api/v1/referral-codes",
    tag = "referral-codes",
    security(("cookie" = [])),
    request_body = NewCode,
    responses(
        (status = 201, body = ReferralCode),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, description = "Code taken, or affiliate not active and enrolled", body = ErrorBody),
    )
)]
pub async fn create_code(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<NewCode>,
) -> Result<(StatusCode, Json<ReferralCode>), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    if bf_db::affiliates::get_affiliate(&s.pool, ws, req.affiliate_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound("affiliate not found"));
    }
    let spec = CodeSpec {
        affiliate_id: req.affiliate_id,
        program_id: req.program_id,
        code: req.code.as_deref(),
        landing_url: req.landing_url.as_deref(),
    };
    insert_code(&s, &auth.principal, ws, spec).await
}

#[derive(Deserialize, IntoParams)]
pub struct CodeFilter {
    affiliate_id: Option<Uuid>,
    program_id: Option<Uuid>,
}

/// List referral codes.
#[utoipa::path(
    get,
    path = "/api/v1/referral-codes",
    tag = "referral-codes",
    security(("cookie" = [])),
    params(CodeFilter, PageQuery),
    responses((status = 200, body = Paged<ReferralCode>), (status = 403, body = ErrorBody))
)]
pub async fn list_codes(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(f): Query<CodeFilter>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<ReferralCode>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rows = bf_db::affiliates::list_codes(
        &s.pool,
        ws,
        f.affiliate_id,
        f.program_id,
        page.limit(),
        page.offset(),
    )
    .await?;
    Ok(Json(page.wrap(rows)))
}

/// Deactivate (delete) a referral code. Notifies the tracker.
#[utoipa::path(
    delete,
    path = "/api/v1/referral-codes/{code}",
    tag = "referral-codes",
    security(("cookie" = [])),
    params(("code" = String, Path, description = "Referral code")),
    responses((status = 204), (status = 404, body = ErrorBody))
)]
pub async fn delete_code(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(code): Path<String>,
) -> Result<StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    remove_code(&s, &auth.principal, ws, &code, None).await
}

#[derive(Deserialize, ToSchema)]
pub struct NewOwnCode {
    program_id: Uuid,
    /// Generated when omitted.
    code: Option<String>,
    /// Defaults to the program's first site.
    landing_url: Option<String>,
}

/// Create a referral code for the signed-in affiliate. Notifies the tracker.
#[utoipa::path(
    post,
    path = "/api/v1/me/referral-codes",
    tag = "me",
    security(("cookie" = [])),
    request_body = NewOwnCode,
    responses(
        (status = 201, body = ReferralCode),
        (status = 400, body = ErrorBody),
        (status = 409, description = "Code taken, or not active and enrolled", body = ErrorBody),
    )
)]
pub async fn me_create_code(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    AppJson(req): AppJson<NewOwnCode>,
) -> Result<(StatusCode, Json<ReferralCode>), AppError> {
    let own = own_affiliate(&s, &auth.principal).await?;
    let spec = CodeSpec {
        affiliate_id: own.id,
        program_id: req.program_id,
        code: req.code.as_deref(),
        landing_url: req.landing_url.as_deref(),
    };
    insert_code(&s, &auth.principal, own.workspace_id, spec).await
}

/// List the signed-in affiliate's referral codes.
#[utoipa::path(
    get,
    path = "/api/v1/me/referral-codes",
    tag = "me",
    security(("cookie" = [])),
    params(PageQuery),
    responses((status = 200, body = Paged<ReferralCode>), (status = 403, body = ErrorBody))
)]
pub async fn me_list_codes(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<ReferralCode>>, AppError> {
    let own = own_affiliate(&s, &auth.principal).await?;
    let rows = bf_db::affiliates::list_codes(
        &s.pool,
        own.workspace_id,
        Some(own.id),
        None,
        page.limit(),
        page.offset(),
    )
    .await?;
    Ok(Json(page.wrap(rows)))
}

/// Deactivate (delete) one of the signed-in affiliate's codes. Notifies the tracker.
#[utoipa::path(
    delete,
    path = "/api/v1/me/referral-codes/{code}",
    tag = "me",
    security(("cookie" = [])),
    params(("code" = String, Path, description = "Referral code")),
    responses((status = 204), (status = 404, body = ErrorBody))
)]
pub async fn me_delete_code(
    State(s): State<AppState>,
    auth: Auth<AffiliateRole>,
    Path(code): Path<String>,
) -> Result<StatusCode, AppError> {
    let own = own_affiliate(&s, &auth.principal).await?;
    remove_code(&s, &auth.principal, own.workspace_id, &code, Some(own.id)).await
}
