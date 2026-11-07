//! `/api/v1/auth/*` and the OpenAPI document.

use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_db::auth::{Rotation, User, UserRole};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{
    ACCESS_COOKIE, ACCESS_TTL_SECS, Any, Auth, CSRF_COOKIE, Principal, REFRESH_COOKIE,
    REFRESH_TTL_SECS, cookie, csrf_set_cookie, dummy_hash, hash_refresh_token, new_csrf_token,
    new_refresh_token, verify_password_bounded,
};
use crate::error::{AppError, AppJson, ErrorBody, ErrorDetail};
use crate::limit;
use crate::{
    admin_reset, affiliates, analytics, billing, invoice, mail, money, notify_channels, ops,
    password_reset, payout_rails, payout_requests, payouts, programs, tax, totp, webhooks,
    workspaces,
};

const REFRESH_PATH: &str = "/api/v1/auth";

struct CookieAuth;

impl Modify for CookieAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(c) = openapi.components.as_mut() {
            c.add_security_scheme(
                "cookie",
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(ACCESS_COOKIE))),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(
        login, refresh, logout, me, csrf,
        password_reset::reset,
        admin_reset::issue,
        admin_reset::reset_with_token,
        totp::enrol, totp::confirm, totp::regenerate, totp::disable, totp::status,
        workspaces::create_workspace, workspaces::list_workspaces, workspaces::signup,
        workspaces::invite_member, workspaces::list_members,
        programs::create_program, programs::list_programs, programs::get_program,
        programs::update_program, programs::delete_program, programs::rotate_secrets,
        programs::add_site, programs::list_sites, programs::delete_site,
        programs::create_conversion_type, programs::list_conversion_types,
        programs::delete_conversion_type,
        programs::create_plan, programs::list_plans, programs::delete_plan,
        affiliates::register, affiliates::list_affiliates, affiliates::get_affiliate,
        affiliates::update_affiliate, affiliates::approve, affiliates::suspend,
        affiliates::reactivate, affiliates::enroll, affiliates::list_enrollments,
        affiliates::unenroll, affiliates::create_code, affiliates::list_codes,
        affiliates::delete_code, affiliates::me_affiliate, affiliates::me_update,
        affiliates::me_create_code, affiliates::me_list_codes, affiliates::me_delete_code,
        analytics::summary, analytics::summary_all, analytics::timeseries, analytics::report_csv,
        webhooks::create_endpoint, webhooks::list_endpoints, webhooks::get_endpoint,
        webhooks::update_endpoint, webhooks::delete_endpoint, webhooks::list_deliveries,
        webhooks::redeliver, webhooks::list_inbound, webhooks::replay_inbound,
        webhooks::test_webhook,
        mail::create_template, mail::list_templates, mail::get_template,
        mail::update_template, mail::delete_template,
        ops::healthz, ops::readyz, crate::alerts::webhook,
        ops::list_notifications, ops::mark_read,
        ops::create_contact, ops::list_contacts, ops::update_contact,
        ops::get_settings, ops::put_settings, ops::list_audit,
        notify_channels::get_channels, notify_channels::put_channels, notify_channels::test_channel,
        money::list_customers, money::get_customer,
        money::list_conversions, money::get_conversion,
        money::approve, money::reject, money::reverse,
        money::list_entries, money::list_balances, money::me_earnings,
        payouts::create_batch_route, payouts::list_batches, payouts::get_batch,
        payouts::approve, payouts::mark_paid_route, payouts::cancel, payouts::export,
        payouts::me_payouts,
        payout_rails::reconcile_route, payout_rails::list_rails, payout_rails::put_credentials,
        payout_rails::delete_credentials,
        payout_requests::me_create, payout_requests::me_list, payout_requests::admin_list,
        payout_requests::approve, payout_requests::reject,
        billing::checkout, billing::subscription, billing::list_billing_plans, billing::list_providers,
        billing::set_provider, billing::webhook,
        tax::me_get, tax::me_put, tax::report_1099, invoice::invoice,
    ),
    components(schemas(
        LoginRequest, UserView, workspaces::SignupRequest, password_reset::ResetRequest, admin_reset::IssuedToken, admin_reset::ResetWithTokenRequest, totp::EnrolRequest, totp::EnrolResponse, totp::CodeRequest,
        totp::RecoveryCodes, totp::TotpStatus, ErrorBody, ErrorDetail, money::ConversionStatus, money::Account,
        billing::CheckoutRequest, billing::CheckoutResponse, billing::SubscriptionView,
        billing::PlanView, billing::UsageView, billing::ProviderView, billing::SetSecret,
        billing::WebhookAck, crate::alerts::AlertAck
    )),
    modifiers(&CookieAuth),
    info(title = "bunfiiliate API", version = "0.1.0")
)]
pub struct ApiDoc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/openapi.json", get(openapi))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
        .route("/api/v1/auth/csrf", get(csrf))
        .merge(password_reset::routes())
        .merge(admin_reset::routes())
        .merge(totp::routes())
        .merge(workspaces::routes())
        .merge(programs::routes())
        .merge(affiliates::routes())
        .merge(analytics::routes())
        .merge(webhooks::routes())
        .merge(mail::routes())
        .merge(ops::routes())
        .merge(notify_channels::routes())
        .merge(crate::alerts::routes())
        .merge(money::routes())
        .merge(payouts::routes())
        .merge(payout_rails::routes())
        .merge(payout_requests::routes())
        .merge(billing::routes())
        .merge(tax::routes())
        .merge(invoice::routes())
}

/// One OpenAPI operation, for the tenant audit inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteInfo {
    pub method: String,
    pub path: String,
    /// Operation declares a security requirement (cookie session).
    pub secured: bool,
    pub path_params: Vec<String>,
}

/// Every operation in the OpenAPI doc, sorted by path then method.
pub fn route_table() -> Vec<RouteInfo> {
    let doc = serde_json::to_value(ApiDoc::openapi()).expect("openapi serialises");
    let mut out = Vec::new();
    for (path, item) in doc["paths"].as_object().into_iter().flatten() {
        for (method, op) in item.as_object().into_iter().flatten() {
            let path_params = op["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["in"] == "path")
                .filter_map(|p| p["name"].as_str().map(str::to_owned))
                .collect();
            out.push(RouteInfo {
                method: method.clone(),
                path: path.clone(),
                secured: op["security"].as_array().is_some_and(|a| !a.is_empty()),
                path_params,
            });
        }
    }
    out.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    out
}

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Serialize, ToSchema)]
pub struct UserView {
    id: Uuid,
    email: String,
    #[schema(value_type = String, example = "admin")]
    role: UserRole,
    workspace_id: Option<Uuid>,
}

impl From<User> for UserView {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            email: u.email,
            role: u.role,
            workspace_id: u.workspace_id,
        }
    }
}

fn set_cookie(name: &str, value: &str, path: &str, max_age: i64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    let c = format!(
        "{name}={value}; HttpOnly; SameSite=Strict; Path={path}; Max-Age={max_age}{secure}"
    );
    HeaderValue::from_str(&c).expect("cookie is ascii")
}

fn session_cookies(s: &AppState, access: &str, refresh: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.append(
        SET_COOKIE,
        set_cookie(
            ACCESS_COOKIE,
            access,
            "/",
            ACCESS_TTL_SECS,
            s.secure_cookies,
        ),
    );
    h.append(
        SET_COOKIE,
        set_cookie(
            REFRESH_COOKIE,
            refresh,
            REFRESH_PATH,
            REFRESH_TTL_SECS,
            s.secure_cookies,
        ),
    );
    h.append(
        SET_COOKIE,
        csrf_set_cookie(&new_csrf_token(), s.secure_cookies),
    );
    h
}

fn clear_cookies(s: &AppState) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.append(
        SET_COOKIE,
        set_cookie(ACCESS_COOKIE, "", "/", 0, s.secure_cookies),
    );
    h.append(
        SET_COOKIE,
        set_cookie(REFRESH_COOKIE, "", REFRESH_PATH, 0, s.secure_cookies),
    );
    h
}

fn refresh_expiry() -> chrono::DateTime<Utc> {
    Utc::now() + Duration::seconds(REFRESH_TTL_SECS)
}

/// Sign in with email and password; sets `bf_at` and `bf_rt` cookies.
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Signed in", body = UserView),
        (status = 401, description = "Bad credentials", body = ErrorBody),
    )
)]
async fn login(
    State(s): State<AppState>,
    headers: HeaderMap,
    AppJson(req): AppJson<LoginRequest>,
) -> Result<(HeaderMap, Json<UserView>), AppError> {
    limit::check(&s.limiter, &limit::LOGIN, &headers, &req.email)?;
    let user = bf_db::auth::user_by_email(&s.pool, &req.email).await?;
    // An unknown email verifies against a dummy hash so it costs the same as a wrong password.
    let hash = user
        .as_ref()
        .map_or_else(|| dummy_hash().to_string(), |u| u.password_hash.clone());
    let ok = verify_password_bounded(req.password, hash).await;
    // Backoff is per (email, address) so it slows a guesser without locking the account out. A
    // backed-off address gets the same 401 as a wrong password, even for the right one.
    let throttle_key = limit::client_ip(&headers)
        .map(|ip| format!("{}|{ip}", req.email.trim().to_ascii_lowercase()));
    let now = Utc::now();
    let backed_off = match &throttle_key {
        Some(k) => bf_db::throttle::is_locked(&s.pool, bf_db::throttle::LOGIN_IP, k, now).await?,
        None => false,
    };
    let user = match user {
        Some(u) if ok && !backed_off => u,
        _ => {
            if let Some(k) = &throttle_key {
                bf_db::throttle::record_failure(
                    &s.pool,
                    bf_db::throttle::LOGIN_IP,
                    k,
                    now,
                    &bf_core::throttle::LOGIN,
                )
                .await?;
            }
            return Err(AppError::Unauthorized("invalid credentials"));
        }
    };
    if let Some(k) = &throttle_key {
        bf_db::throttle::clear(&s.pool, bf_db::throttle::LOGIN_IP, k).await?;
    }
    let cookies = start_session(&s, &user).await?;
    Ok((cookies, Json(user.into())))
}

/// Issue the access + refresh cookies for `user` and audit the login. Shared by login and signup.
pub(crate) async fn start_session(s: &AppState, user: &User) -> Result<HeaderMap, AppError> {
    let principal = Principal::from(user);
    let ver = bf_db::auth::token_version(&s.pool, user.id)
        .await?
        .unwrap_or_default();
    let access = s.jwt_keys.issue(&principal, ver)?;
    let refresh = new_refresh_token();
    bf_db::auth::issue_refresh(
        &s.pool,
        user.id,
        &hash_refresh_token(&refresh),
        refresh_expiry(),
    )
    .await?;
    audit::record(&s.pool, &principal, "auth.login", "user", user.id, None).await?;
    Ok(session_cookies(s, &access, &refresh))
}

/// Exchange the `bf_rt` cookie for a fresh pair. Reusing an old token revokes the family.
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    tag = "auth",
    responses(
        (status = 200, description = "Rotated", body = UserView),
        (status = 401, description = "Missing, expired or reused token", body = ErrorBody),
    )
)]
async fn refresh(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<UserView>), AppError> {
    let old =
        cookie(&headers, REFRESH_COOKIE).ok_or(AppError::Unauthorized("missing refresh token"))?;
    let new = new_refresh_token();
    let rotation = bf_db::auth::rotate_refresh(
        &s.pool,
        &hash_refresh_token(old),
        &hash_refresh_token(&new),
        refresh_expiry(),
    )
    .await?;
    let user_id = match rotation {
        Rotation::Rotated { user_id } => user_id,
        Rotation::Reuse { user_id } => {
            if let Some(user) = bf_db::auth::user_by_id(&s.pool, user_id).await? {
                audit::record(
                    &s.pool,
                    &Principal::from(&user),
                    "auth.refresh_reuse",
                    "user",
                    user_id,
                    None,
                )
                .await?;
            }
            return Err(AppError::Unauthorized("refresh token reused"));
        }
        Rotation::Grace | Rotation::Invalid => {
            return Err(AppError::Unauthorized("invalid refresh token"));
        }
    };
    let user = bf_db::auth::user_by_id(&s.pool, user_id)
        .await?
        .ok_or(AppError::Unauthorized("invalid refresh token"))?;
    let ver = bf_db::auth::token_version(&s.pool, user.id)
        .await?
        .unwrap_or_default();
    let access = s.jwt_keys.issue(&Principal::from(&user), ver)?;
    Ok((session_cookies(&s, &access, &new), Json(user.into())))
}

#[derive(Serialize, ToSchema)]
pub struct CsrfToken {
    token: String,
}

/// CSRF token for the double-submit check; sets the `bf_csrf` cookie if missing.
/// Send the token back as `X-CSRF-Token` on every mutating request, login included.
#[utoipa::path(
    get,
    path = "/api/v1/auth/csrf",
    tag = "auth",
    responses((status = 200, body = CsrfToken))
)]
async fn csrf(State(s): State<AppState>, headers: HeaderMap) -> (HeaderMap, Json<CsrfToken>) {
    let mut out = HeaderMap::new();
    let token = match cookie(&headers, CSRF_COOKIE).filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => {
            let t = new_csrf_token();
            out.append(SET_COOKIE, csrf_set_cookie(&t, s.secure_cookies));
            t
        }
    };
    (out, Json(CsrfToken { token }))
}

/// Revoke the refresh-token family and clear both cookies.
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses((status = 204, description = "Signed out"))
)]
async fn logout(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, HeaderMap), AppError> {
    if let Some(token) = cookie(&headers, REFRESH_COOKIE) {
        bf_db::auth::revoke_family_of(&s.pool, &hash_refresh_token(token)).await?;
    }
    Ok((StatusCode::NO_CONTENT, clear_cookies(&s)))
}

/// The signed-in user.
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    security(("cookie" = [])),
    responses(
        (status = 200, body = UserView),
        (status = 401, body = ErrorBody),
    )
)]
async fn me(State(s): State<AppState>, auth: Auth<Any>) -> Result<Json<UserView>, AppError> {
    let user = bf_db::auth::user_by_id(&s.pool, auth.principal.user_id)
        .await?
        .ok_or(AppError::Unauthorized("user no longer exists"))?;
    Ok(Json(user.into()))
}
