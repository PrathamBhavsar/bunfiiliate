//! Alertmanager webhook sink. Enabled only when `BF_ALERT_TOKEN` is set; the caller sends it as
//! `Authorization: Bearer <token>`. Alerts go to super admins as in-app notifications.

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

use crate::AppState;
use crate::error::{AppError, ErrorBody};

/// Alertmanager groups can be large, but not this large.
const MAX_BODY: usize = 64 * 1024;
const MAX_ALERTS: usize = 50;
const MAX_FIELD: usize = 300;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/alerts/webhook",
        post(webhook).layer(DefaultBodyLimit::max(MAX_BODY)),
    )
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    alerts: Vec<Alert>,
}

#[derive(Deserialize)]
struct Alert {
    #[serde(default)]
    status: String,
    #[serde(default)]
    labels: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    annotations: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize, ToSchema)]
pub struct AlertAck {
    /// Alerts accepted from the payload.
    alerts: usize,
    /// Super admins each alert was delivered to.
    recipients: usize,
}

/// Does `header` carry `configured` as a bearer token? `None` configured never matches.
/// Compares digests-length-independently in constant time.
pub fn token_ok(configured: Option<&str>, header: Option<&str>) -> bool {
    let (Some(want), Some(got)) = (configured, header) else {
        return false;
    };
    if want.is_empty() {
        return false;
    }
    let got = got.strip_prefix("Bearer ").unwrap_or("");
    use sha2::{Digest, Sha256};
    let (a, b) = (Sha256::digest(want), Sha256::digest(got));
    bool::from(a.ct_eq(&b))
}

fn clip(s: &str) -> String {
    s.chars().take(MAX_FIELD).collect()
}

/// Alertmanager webhook receiver. Public route: authenticated by `BF_ALERT_TOKEN`; 404 when
/// the token is not configured.
#[utoipa::path(
    post,
    path = "/api/v1/alerts/webhook",
    tag = "ops",
    responses(
        (status = 200, body = AlertAck),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 413, body = ErrorBody),
    ),
)]
pub async fn webhook(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<AlertAck>, AppError> {
    let configured = std::env::var("BF_ALERT_TOKEN").ok();
    if configured.as_deref().is_none_or(str::is_empty) {
        return Err(AppError::NotFound("alerts are not enabled"));
    }
    let header = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    if !token_ok(configured.as_deref(), header) {
        return Err(AppError::Unauthorized("bad alert token"));
    }
    let payload: Payload =
        serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("invalid json".into()))?;
    let mut recipients = 0;
    let alerts = payload.alerts.len().min(MAX_ALERTS);
    for a in payload.alerts.iter().take(MAX_ALERTS) {
        let name = clip(a.labels.get("alertname").map_or("alert", String::as_str));
        let severity = clip(a.labels.get("severity").map_or("", String::as_str));
        let summary = clip(
            a.annotations
                .get("summary")
                .or_else(|| a.annotations.get("description"))
                .map_or("", String::as_str),
        );
        let firing = a.status != "resolved";
        let prefix = format!("[{name}]");
        let text = format!(
            "{prefix} {} {severity} {summary}",
            if firing { "firing" } else { "resolved" }
        );
        recipients =
            bf_db::ops::notify_super_admins_alert(&s.pool, firing, &prefix, text.trim()).await?;
    }
    Ok(Json(AlertAck { alerts, recipients }))
}
