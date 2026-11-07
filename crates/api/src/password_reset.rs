//! `POST /api/v1/auth/password/reset`: set a new password with a TOTP or recovery code.
//!
//! Every failure after the password check (unknown email, no TOTP, bad or replayed code) returns
//! the same 401. The new password is validated before any code is consumed, and the code, the
//! password change, session revocation, notification and audit row commit in one transaction.
//! Neither the code nor the password is ever logged.

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use bf_core::totp;
use chrono::Utc;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Principal, dummy_hash, hash_password_bounded, verify_password_bounded};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::limit;

const MIN_NEW_PASSWORD: usize = 12;
const MAX_NEW_PASSWORD: usize = 256;
/// Recovery verification always costs this many argon2 runs, whatever the user has left.
const RECOVERY_SLOTS: usize = 10;
const FAILED: &str = "password reset failed";

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/auth/password/reset", post(reset))
}

#[derive(Deserialize, ToSchema)]
pub struct ResetRequest {
    pub email: String,
    /// Six-digit authenticator code, or a recovery code (`xxxxx-xxxxx`).
    pub code: String,
    /// 12 to 256 characters.
    pub new_password: String,
}

enum Proof {
    Step(i64),
    Recovery(Uuid),
}

/// Reset the password of an account that has TOTP enrolled.
#[utoipa::path(
    post,
    path = "/api/v1/auth/password/reset",
    tag = "auth",
    request_body = ResetRequest,
    responses(
        (status = 204, description = "Password changed; every session revoked"),
        (status = 400, description = "New password too short or too long", body = ErrorBody),
        (status = 401, description = "Reset failed (same body for every cause)", body = ErrorBody),
        (status = 429, description = "Too many requests", body = ErrorBody),
    )
)]
pub async fn reset(
    State(s): State<AppState>,
    headers: HeaderMap,
    AppJson(req): AppJson<ResetRequest>,
) -> Result<StatusCode, AppError> {
    limit::check(&s.limiter, &limit::RESET, &headers, &req.email)?;
    // Validated first: a weak password never costs the user a code, and says nothing about them.
    let n = req.new_password.chars().count();
    if !(MIN_NEW_PASSWORD..=MAX_NEW_PASSWORD).contains(&n) {
        return Err(AppError::BadRequest(format!(
            "password must be {MIN_NEW_PASSWORD} to {MAX_NEW_PASSWORD} characters"
        )));
    }
    let email = req.email.trim();
    let user = bf_db::auth::user_by_email(&s.pool, email).await?;
    let totp_row = match &user {
        Some(u) => bf_db::totp::get(&s.pool, u.id)
            .await?
            .filter(|t| t.confirmed),
        None => None,
    };
    let code = req.code.trim();
    let is_totp = code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit());
    let proof = match (&user, &totp_row) {
        (Some(u), Some(t)) => verify_code(&s, u.id, t, code, is_totp).await?,
        _ => {
            dummy_verify(is_totp).await;
            None
        }
    };
    let (Some(user), Some(proof)) = (user, proof) else {
        return Err(AppError::Unauthorized(FAILED));
    };
    let hash = hash_password_bounded(req.new_password).await?;

    let mut tx = s.pool.begin().await?;
    let claimed = match proof {
        Proof::Step(step) => bf_db::totp::claim_step(&mut *tx, user.id, step).await?,
        Proof::Recovery(id) => bf_db::totp::consume_recovery_code(&mut *tx, user.id, id).await?,
    };
    if !claimed {
        return Err(AppError::Unauthorized(FAILED));
    }
    bf_db::auth::set_password_hash(&mut *tx, user.id, &hash).await?;
    bf_db::auth::delete_refresh_tokens(&mut *tx, user.id).await?;
    bf_db::auth::bump_token_version(&mut *tx, user.id).await?;
    bf_db::auth::insert_notification(
        &mut *tx,
        user.id,
        "password_changed",
        "Your password was changed with a verification code. If this was not you, contact an admin.",
    )
    .await?;
    let actor = Principal::from(&user);
    audit::record(
        &mut *tx,
        &actor,
        "auth.password_reset",
        "user",
        user.id,
        None,
    )
    .await?;
    tx.commit().await?;
    // After commit, so a request cannot re-cache the old version once the new one is visible.
    s.version_cache.evict(user.id);
    Ok(StatusCode::NO_CONTENT)
}

/// Check `code` against the user's TOTP secret or recovery codes. `None` = not proven.
async fn verify_code(
    s: &AppState,
    user_id: Uuid,
    row: &bf_db::totp::Totp,
    code: &str,
    is_totp: bool,
) -> Result<Option<Proof>, AppError> {
    if is_totp {
        let b32 = s
            .vault
            .open_col("user_totp", "secret_enc", user_id, &row.secret_enc)
            .map_err(|e| {
                tracing::error!(user = %user_id, error = %e, "cannot open totp secret");
                AppError::Internal
            })?;
        let secret = data_encoding::BASE32_NOPAD
            .decode(b32.as_bytes())
            .map_err(|_| AppError::Internal)?;
        return Ok(
            totp::verify(&secret, code, Utc::now().timestamp(), row.last_used_step)
                .map(Proof::Step),
        );
    }
    let wanted = totp::normalize_recovery(code);
    let codes = bf_db::totp::unused_recovery_codes(&s.pool, user_id).await?;
    let mut found = None;
    let mut runs = 0;
    for (id, hash) in codes {
        runs += 1;
        if verify_password_bounded(wanted.clone(), hash).await && found.is_none() {
            found = Some(id);
        }
    }
    while runs < RECOVERY_SLOTS {
        runs += 1;
        verify_password_bounded(wanted.clone(), dummy_hash().to_string()).await;
    }
    Ok(found.map(Proof::Recovery))
}

/// Same work as [`verify_code`] for an account that cannot be reset, so timing does not tell.
async fn dummy_verify(is_totp: bool) {
    if is_totp {
        let _ = totp::verify(&[0u8; 20], "000000", Utc::now().timestamp(), None);
        return;
    }
    for _ in 0..RECOVERY_SLOTS {
        verify_password_bounded(String::new(), dummy_hash().to_string()).await;
    }
}
