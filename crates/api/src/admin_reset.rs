//! Admin-assisted password reset: an admin issues a one-hour token, the user redeems it.
//!
//! Issuing: a workspace admin may target affiliates in their own workspace; a super admin
//! anyone but themselves; nobody targets themselves. Another workspace is a 404. Only the
//! SHA-256 of a token is stored; the token is shown once. Redeeming has the same side effects
//! as the TOTP reset (sessions revoked, token version bumped, notification, audit) and the
//! same generic 401 for any bad token.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bf_db::auth::UserRole;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Auth, Principal, hash_password_bounded, hash_refresh_token};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::limit;
use crate::notify::{self, Notifier};

const TTL_MINUTES: i64 = 60;
const MIN_NEW_PASSWORD: usize = 12;
const MAX_NEW_PASSWORD: usize = 256;
const FAILED: &str = "password reset failed";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/users/{id}/password-reset-token", post(issue))
        .route(
            "/api/v1/auth/password/reset-with-token",
            post(reset_with_token),
        )
}

#[derive(Serialize, ToSchema)]
pub struct IssuedToken {
    /// Shown once.
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// Issue a one-hour password reset token for a user.
#[utoipa::path(
    post,
    path = "/api/v1/users/{id}/password-reset-token",
    tag = "auth",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "User id")),
    responses(
        (status = 201, body = IssuedToken),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
pub async fn issue(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<IssuedToken>), AppError> {
    let actor = auth.principal;
    let target = bf_db::auth::user_by_id(&s.pool, id)
        .await?
        .ok_or(AppError::NotFound("user not found"))?;
    if actor.role != UserRole::SuperAdmin && target.workspace_id != actor.workspace_id {
        return Err(AppError::NotFound("user not found"));
    }
    if target.id == actor.user_id {
        return Err(AppError::Forbidden(
            "use the normal reset flow for yourself",
        ));
    }
    if actor.role != UserRole::SuperAdmin && target.role != UserRole::Affiliate {
        return Err(AppError::Forbidden("cannot reset this account"));
    }

    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let token = URL_SAFE_NO_PAD.encode(raw);
    let expires_at = Utc::now() + Duration::minutes(TTL_MINUTES);
    let mut tx = s.pool.begin().await?;
    bf_db::reset_tokens::replace(
        &mut tx,
        target.id,
        actor.user_id,
        &hash_refresh_token(&token),
        expires_at,
    )
    .await?;
    audit::record(
        &mut *tx,
        &actor,
        "auth.reset_token_issued",
        "user",
        target.id,
        Some(json!({"expires_at": expires_at})),
    )
    .await?;
    tx.commit().await?;
    // Best effort: the audit row is the record, the notice is a courtesy.
    let _ = notify::from_env(&s.pool)
        .notify(
            target.id,
            "password_reset_token_issued",
            "A password reset was issued for your account",
            "An admin issued a password reset token for your account. If you did not ask for this, contact an admin.",
        )
        .await;
    Ok((StatusCode::CREATED, Json(IssuedToken { token, expires_at })))
}

#[derive(Deserialize, ToSchema)]
pub struct ResetWithTokenRequest {
    pub token: String,
    /// 12 to 256 characters.
    pub new_password: String,
}

/// Set a new password with an admin-issued token.
#[utoipa::path(
    post,
    path = "/api/v1/auth/password/reset-with-token",
    tag = "auth",
    request_body = ResetWithTokenRequest,
    responses(
        (status = 204, description = "Password changed; every session revoked"),
        (status = 400, description = "New password too short or too long", body = ErrorBody),
        (status = 401, description = "Reset failed (same body for every cause)", body = ErrorBody),
        (status = 429, description = "Too many requests", body = ErrorBody),
    )
)]
pub async fn reset_with_token(
    State(s): State<AppState>,
    headers: HeaderMap,
    AppJson(req): AppJson<ResetWithTokenRequest>,
) -> Result<StatusCode, AppError> {
    let hash = hash_refresh_token(req.token.trim());
    let key: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
    limit::check(&s.limiter, &limit::RESET, &headers, &key)?;
    let n = req.new_password.chars().count();
    if !(MIN_NEW_PASSWORD..=MAX_NEW_PASSWORD).contains(&n) {
        return Err(AppError::BadRequest(format!(
            "password must be {MIN_NEW_PASSWORD} to {MAX_NEW_PASSWORD} characters"
        )));
    }
    let new_hash = hash_password_bounded(req.new_password).await?;

    let mut tx = s.pool.begin().await?;
    let user_id = bf_db::reset_tokens::claim(&mut tx, &hash)
        .await?
        .ok_or(AppError::Unauthorized(FAILED))?;
    let user = bf_db::auth::user_by_id(&mut *tx, user_id)
        .await?
        .ok_or(AppError::Unauthorized(FAILED))?;
    bf_db::auth::set_password_hash(&mut *tx, user.id, &new_hash).await?;
    bf_db::auth::delete_refresh_tokens(&mut *tx, user.id).await?;
    bf_db::auth::bump_token_version(&mut *tx, user.id).await?;
    bf_db::auth::insert_notification(
        &mut *tx,
        user.id,
        "password_changed",
        "Your password was changed with an admin-issued reset token. If this was not you, contact an admin.",
    )
    .await?;
    audit::record(
        &mut *tx,
        &Principal::from(&user),
        "auth.reset_token_used",
        "user",
        user.id,
        None,
    )
    .await?;
    tx.commit().await?;
    s.version_cache.evict(user.id);
    if let Some(mail) = notify::mail_from_env(&s.pool) {
        let _ = mail
            .notify(
                user.id,
                "password_changed",
                "Your password was changed",
                "Your password was changed with an admin-issued reset token. If this was not you, contact an admin.",
            )
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}
