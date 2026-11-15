//! `/api/v1/auth/totp/*`: enrol, confirm, regenerate recovery codes, disable, status.
//!
//! The TOTP secret is sealed with the vault (AAD binds it to the user) and never returned after
//! enrol. Recovery codes are shown once and stored as salted argon2 hashes.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_core::totp;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Any, Auth, Principal, hash_password_bounded, verify_password_bounded};
use crate::error::{AppError, AppJson, ErrorBody};

const TABLE: &str = "user_totp";
const COLUMN: &str = "secret_enc";
const ISSUER: &str = "bunfiiliate";
const RECOVERY_CODES: usize = 10;
const BAD_CREDENTIALS: &str = "invalid password or code";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/totp/enrol", post(enrol))
        .route("/api/v1/auth/totp/confirm", post(confirm))
        .route(
            "/api/v1/auth/totp/recovery-codes/regenerate",
            post(regenerate),
        )
        .route("/api/v1/auth/totp/disable", post(disable))
        .route("/api/v1/auth/totp/status", get(status))
}

#[derive(Deserialize, ToSchema)]
pub struct EnrolRequest {}

#[derive(Serialize, ToSchema)]
pub struct EnrolResponse {
    pub otpauth_uri: String,
    pub secret_base32: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CodeRequest {
    pub current_password: String,
    /// Six-digit authenticator code.
    pub code: String,
}

#[derive(Serialize, ToSchema)]
pub struct RecoveryCodes {
    /// Shown once; only salted hashes are stored.
    pub recovery_codes: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct TotpStatus {
    pub enrolled: bool,
    pub recovery_codes_remaining: i64,
}

/// Start enrolment: returns the otpauth URI and secret. Replaces any unconfirmed secret.
#[utoipa::path(
    post,
    path = "/api/v1/auth/totp/enrol",
    tag = "auth",
    security(("cookie" = [])),
    request_body = EnrolRequest,
    responses(
        (status = 200, body = EnrolResponse),
        (status = 401, body = ErrorBody),
        (status = 409, description = "Already enrolled", body = ErrorBody),
    )
)]
pub async fn enrol(
    State(s): State<AppState>,
    auth: Auth<Any>,
    AppJson(_): AppJson<EnrolRequest>,
) -> Result<Json<EnrolResponse>, AppError> {
    let user = user_row(&s, &auth.principal).await?;
    let secret = totp::generate_secret();
    let b32 = totp::base32(&secret);
    let sealed = s.vault.seal_col(TABLE, COLUMN, user.id, &b32);
    let stored = bf_db::totp::upsert_pending(&s.pool, user.id, &sealed, sealed != b32).await?;
    if !stored {
        return Err(AppError::Conflict("totp already enrolled"));
    }
    Ok(Json(EnrolResponse {
        otpauth_uri: totp::otpauth_uri(ISSUER, &user.email, &secret),
        secret_base32: b32,
    }))
}

/// Finish enrolment with a current code and the password; returns the recovery codes once.
#[utoipa::path(
    post,
    path = "/api/v1/auth/totp/confirm",
    tag = "auth",
    security(("cookie" = [])),
    request_body = CodeRequest,
    responses(
        (status = 200, body = RecoveryCodes),
        (status = 401, description = "Wrong password or code", body = ErrorBody),
        (status = 409, description = "Already enrolled", body = ErrorBody),
    )
)]
pub async fn confirm(
    State(s): State<AppState>,
    auth: Auth<Any>,
    AppJson(req): AppJson<CodeRequest>,
) -> Result<Json<RecoveryCodes>, AppError> {
    let p = &auth.principal;
    check_password(&s, p, &req.current_password).await?;
    let row = bf_db::totp::get(&s.pool, p.user_id)
        .await?
        .ok_or(AppError::BadRequest("start enrolment first".into()))?;
    if row.confirmed {
        return Err(AppError::Conflict("totp already enrolled"));
    }
    let step = matching_step(&s, &row, &req.code)?;
    let (codes, hashes) = new_codes().await?;
    let mut tx = s.pool.begin().await?;
    if !bf_db::totp::claim_step(&mut *tx, p.user_id, step).await?
        || !bf_db::totp::confirm(&mut *tx, p.user_id).await?
    {
        return Err(AppError::Unauthorized(BAD_CREDENTIALS));
    }
    bf_db::totp::replace_recovery_codes(&mut tx, p.user_id, &hashes).await?;
    audit::record(&mut *tx, p, "totp.enable", "user", p.user_id, None).await?;
    tx.commit().await?;
    Ok(Json(RecoveryCodes {
        recovery_codes: codes,
    }))
}

/// Replace all recovery codes with ten new ones; the old ones stop working.
#[utoipa::path(
    post,
    path = "/api/v1/auth/totp/recovery-codes/regenerate",
    tag = "auth",
    security(("cookie" = [])),
    request_body = CodeRequest,
    responses(
        (status = 200, body = RecoveryCodes),
        (status = 401, description = "Wrong password or code", body = ErrorBody),
        (status = 404, description = "Not enrolled", body = ErrorBody),
    )
)]
pub async fn regenerate(
    State(s): State<AppState>,
    auth: Auth<Any>,
    AppJson(req): AppJson<CodeRequest>,
) -> Result<Json<RecoveryCodes>, AppError> {
    let p = &auth.principal;
    let mut tx = s.pool.begin().await?;
    authorize(&s, &mut tx, p, &req).await?;
    let (codes, hashes) = new_codes().await?;
    bf_db::totp::replace_recovery_codes(&mut tx, p.user_id, &hashes).await?;
    audit::record(
        &mut *tx,
        p,
        "totp.recovery_regenerate",
        "user",
        p.user_id,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(RecoveryCodes {
        recovery_codes: codes,
    }))
}

/// Turn TOTP off and delete the secret and recovery codes.
#[utoipa::path(
    post,
    path = "/api/v1/auth/totp/disable",
    tag = "auth",
    security(("cookie" = [])),
    request_body = CodeRequest,
    responses(
        (status = 204, description = "Disabled"),
        (status = 401, description = "Wrong password or code", body = ErrorBody),
        (status = 404, description = "Not enrolled", body = ErrorBody),
    )
)]
pub async fn disable(
    State(s): State<AppState>,
    auth: Auth<Any>,
    AppJson(req): AppJson<CodeRequest>,
) -> Result<axum::http::StatusCode, AppError> {
    let p = &auth.principal;
    let mut tx = s.pool.begin().await?;
    authorize(&s, &mut tx, p, &req).await?;
    bf_db::totp::delete(&mut *tx, p.user_id).await?;
    bf_db::totp::delete_recovery_codes(&mut *tx, p.user_id).await?;
    audit::record(&mut *tx, p, "totp.disable", "user", p.user_id, None).await?;
    tx.commit().await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Whether TOTP is enrolled and how many recovery codes are left.
#[utoipa::path(
    get,
    path = "/api/v1/auth/totp/status",
    tag = "auth",
    security(("cookie" = [])),
    responses((status = 200, body = TotpStatus), (status = 401, body = ErrorBody))
)]
pub async fn status(
    State(s): State<AppState>,
    auth: Auth<Any>,
) -> Result<Json<TotpStatus>, AppError> {
    let id = auth.principal.user_id;
    let enrolled = bf_db::totp::get(&s.pool, id)
        .await?
        .is_some_and(|t| t.confirmed);
    let recovery_codes_remaining = if enrolled {
        bf_db::totp::recovery_codes_remaining(&s.pool, id).await?
    } else {
        0
    };
    Ok(Json(TotpStatus {
        enrolled,
        recovery_codes_remaining,
    }))
}

async fn user_row(s: &AppState, p: &Principal) -> Result<bf_db::auth::User, AppError> {
    bf_db::auth::user_by_id(&s.pool, p.user_id)
        .await?
        .ok_or(AppError::Unauthorized("user no longer exists"))
}

async fn check_password(s: &AppState, p: &Principal, password: &str) -> Result<(), AppError> {
    let user = user_row(s, p).await?;
    if verify_password_bounded(password.to_string(), user.password_hash).await {
        Ok(())
    } else {
        Err(AppError::Unauthorized(BAD_CREDENTIALS))
    }
}

/// Step of `code` against the stored secret, or 401. Does not record the step.
fn matching_step(s: &AppState, row: &bf_db::totp::Totp, code: &str) -> Result<i64, AppError> {
    let secret = open_secret(s, row.user_id, &row.secret_enc)?;
    totp::verify(&secret, code, Utc::now().timestamp(), row.last_used_step)
        .ok_or(AppError::Unauthorized(BAD_CREDENTIALS))
}

fn open_secret(s: &AppState, user_id: Uuid, stored: &str) -> Result<Vec<u8>, AppError> {
    let b32 = s
        .vault
        .open_col(TABLE, COLUMN, user_id, stored)
        .map_err(|e| {
            tracing::error!(user = %user_id, error = %e, "cannot open totp secret");
            AppError::Internal
        })?;
    data_encoding::BASE32_NOPAD
        .decode(b32.as_bytes())
        .map_err(|_| AppError::Internal)
}

/// Password + code for an already-confirmed enrolment; claims the step so a code works once.
async fn authorize(
    s: &AppState,
    tx: &mut sqlx::PgConnection,
    p: &Principal,
    req: &CodeRequest,
) -> Result<(), AppError> {
    check_password(s, p, &req.current_password).await?;
    let row = bf_db::totp::get(&mut *tx, p.user_id)
        .await?
        .filter(|r| r.confirmed)
        .ok_or(AppError::NotFound("totp not enrolled"))?;
    let step = matching_step(s, &row, &req.code)?;
    if !bf_db::totp::claim_step(&mut *tx, p.user_id, step).await? {
        return Err(AppError::Unauthorized(BAD_CREDENTIALS));
    }
    Ok(())
}

/// Fresh recovery codes and their argon2 hashes (hashed on the bounded blocking pool).
async fn new_codes() -> Result<(Vec<String>, Vec<String>), AppError> {
    let codes = totp::new_recovery_codes(RECOVERY_CODES);
    let mut hashes = Vec::with_capacity(codes.len());
    for c in &codes {
        hashes.push(hash_password_bounded(totp::normalize_recovery(c)).await?);
    }
    Ok((codes, hashes))
}
