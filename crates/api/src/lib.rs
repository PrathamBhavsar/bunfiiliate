//! bunfiiliate API: auth, roles, OpenAPI.

pub mod admin_reset;
pub mod affiliates;
pub mod analytics;
pub mod audit;
pub mod auth;
pub mod bootstrap;
pub mod dispatch;
pub mod error;
pub mod headers;
pub mod limit;
pub mod mail;
pub mod metrics;
pub mod money;
pub mod notify;
pub mod ops;
pub mod page;
pub mod password_reset;
pub mod payouts;
pub mod programs;
pub mod routes;
pub mod secrets;
pub mod ssrf;
pub mod totp;
pub mod webhooks;
pub mod worker;
pub mod workspaces;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jwt_keys: Arc<auth::JwtKeys>,
    /// Per-instance cache of `users.token_version`.
    pub version_cache: Arc<auth::VersionCache>,
    /// Master keys for secrets at rest.
    pub vault: Arc<bf_core::secrets::Vault>,
    /// Add `Secure` to cookies; turn off only for plain-http local dev.
    pub secure_cookies: bool,
    /// Rate limits for the public routes; per process, shared by every clone of the state.
    pub limiter: Arc<limit::Limiter>,
}

/// Largest request body any API route accepts.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// Wrap routes with security headers, the Origin check, the CSRF check, the body limit and state.
pub fn build(routes: Router<AppState>, state: AppState) -> Router {
    routes
        .layer(from_fn(auth::check_csrf))
        .layer(from_fn(auth::check_origin))
        .layer(from_fn(headers::security_headers))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

pub fn app(state: AppState) -> Router {
    build(routes::routes(), state)
}
