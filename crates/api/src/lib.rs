//! bunfiiliate API: auth, roles, OpenAPI.

pub mod affiliates;
pub mod analytics;
pub mod audit;
pub mod auth;
pub mod bootstrap;
pub mod dispatch;
pub mod error;
pub mod mail;
pub mod metrics;
pub mod ops;
pub mod page;
pub mod payouts;
pub mod programs;
pub mod routes;
pub mod webhooks;
pub mod worker;
pub mod workspaces;

use std::sync::Arc;

use axum::Router;
use axum::middleware::from_fn;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jwt_secret: Arc<str>,
    /// Add `Secure` to cookies; turn off only for plain-http local dev.
    pub secure_cookies: bool,
}

/// Wrap routes with the Origin check and state.
pub fn build(routes: Router<AppState>, state: AppState) -> Router {
    routes.layer(from_fn(auth::check_origin)).with_state(state)
}

pub fn app(state: AppState) -> Router {
    build(routes::routes(), state)
}
