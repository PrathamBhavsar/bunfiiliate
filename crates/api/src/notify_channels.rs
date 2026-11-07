//! B07 notification channels: a Telegram and a Discord webhook URL per workspace.
//!
//! Both URLs carry a bot token, so they are sealed in `workspaces` (`Vault::seal_col`), listed in
//! `secrets::PROTECTED`, and never returned: reads only say whether one is set. A URL is checked by
//! the SSRF guard when saved, and again at connect time by the shared HTTP client.
//!
//! The dispatcher sends a short plain-text message for [`DEFAULT_TOPICS`] right after the outbox
//! row commits. Like the email, it is best effort (logged, not retried).

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_core::secrets::Vault;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::audit;
use crate::auth::{Admin, Auth};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::{AppState, ssrf};

/// Outbox topics that reach the channels.
pub const DEFAULT_TOPICS: [&str; 4] = [
    "payout.paid",
    "payout.rejected",
    "conversion.reversed",
    "payout_request.created",
];

const TABLE: &str = "workspaces";
const MAX_URL: usize = 2048;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/settings/notify-channels",
            get(get_channels).put(put_channels),
        )
        .route("/api/v1/settings/test-notify-channel", post(test_channel))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Telegram,
    Discord,
}

impl Channel {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "telegram" => Some(Self::Telegram),
            "discord" => Some(Self::Discord),
            _ => None,
        }
    }

    fn column(self) -> &'static str {
        match self {
            Self::Telegram => "notify_telegram_webhook",
            Self::Discord => "notify_discord_webhook",
        }
    }

    /// Telegram expects `{"text"}`, Discord `{"content"}`.
    fn body(self, text: &str) -> Value {
        match self {
            Self::Telegram => json!({ "text": text }),
            Self::Discord => json!({ "content": text }),
        }
    }
}

/// What the dispatcher needs to send: the shared client and the vault to open URLs.
#[derive(Clone)]
pub struct Channels {
    http: reqwest::Client,
    vault: Arc<Vault>,
}

impl Channels {
    pub fn new(http: reqwest::Client, vault: Arc<Vault>) -> Self {
        Self { http, vault }
    }

    /// POST `text` to one channel URL.
    async fn send(&self, ch: Channel, url: &str, text: &str) -> Result<(), String> {
        ssrf::check_literal_host(url).map_err(|_| "url points at a private address".to_string())?;
        let res = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(ch.body(text).to_string())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(format!("channel answered {}", res.status()))
        }
    }

    /// Unsealed configured URLs of a workspace, as `(channel, url)`.
    async fn urls(&self, pool: &PgPool, ws: Uuid) -> Result<Vec<(Channel, String)>, sqlx::Error> {
        let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT notify_telegram_webhook, notify_discord_webhook FROM workspaces WHERE id = $1",
        )
        .bind(ws)
        .fetch_optional(pool)
        .await?;
        let (t, d) = row.unwrap_or_default();
        let mut out = Vec::new();
        for (ch, stored) in [(Channel::Telegram, t), (Channel::Discord, d)] {
            let Some(stored) = stored.filter(|s| !s.is_empty()) else {
                continue;
            };
            match self.vault.open_col(TABLE, ch.column(), ws, &stored) {
                Ok(url) => out.push((ch, url)),
                Err(e) => tracing::warn!(workspace = %ws, error = %e, "cannot open notify url"),
            }
        }
        Ok(out)
    }
}

/// Plain-text summary of an outbox event, or `None` when the topic is not forwarded.
pub fn summary(topic: &str, payload: &Value) -> Option<String> {
    if !DEFAULT_TOPICS.contains(&topic) {
        return None;
    }
    let label = match topic {
        "payout.paid" => "Payout paid",
        "payout.rejected" => "Payout rejected",
        "conversion.reversed" => "Conversion reversed",
        _ => "New payout request",
    };
    let mut parts = vec![label.to_string()];
    let field = |k: &str| match payload.get(k)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    for (key, name) in [
        ("payout_id", "payout"),
        ("conversion_id", "conversion"),
        ("request_id", "request"),
    ] {
        if let Some(v) = field(key) {
            parts.push(format!("{name} {v}"));
        }
    }
    if let Some(c) = field("amount_cents").or_else(|| field("commission_cents")) {
        let cur = field("currency").unwrap_or_default();
        parts.push(format!("{c} cents {cur}").trim_end().to_string());
    }
    if let Some(r) = field("reason") {
        parts.push(format!("reason: {r}"));
    }
    Some(parts.join(" - "))
}

/// Fan an event out to the workspace's configured channels. Sends run detached and are bounded
/// by the client timeout; failures are logged.
pub async fn forward(
    channels: &Channels,
    pool: &PgPool,
    program: Uuid,
    topic: &str,
    payload: &Value,
) -> Result<(), sqlx::Error> {
    let Some(text) = summary(topic, payload) else {
        return Ok(());
    };
    let ws: Option<Uuid> = sqlx::query_scalar("SELECT workspace_id FROM programs WHERE id = $1")
        .bind(program)
        .fetch_optional(pool)
        .await?;
    let Some(ws) = ws else { return Ok(()) };
    for (ch, url) in channels.urls(pool, ws).await? {
        let (channels, text) = (channels.clone(), text.clone());
        tokio::spawn(async move {
            if let Err(e) = channels.send(ch, &url, &text).await {
                tracing::warn!(channel = ?ch, error = %e, "notify channel send failed");
            }
        });
    }
    Ok(())
}

// ---- routes

#[derive(Serialize, ToSchema)]
pub struct ChannelsView {
    /// The URL itself is write-only.
    telegram_set: bool,
    discord_set: bool,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PutChannels {
    /// Absent: unchanged. `null` or empty: cleared.
    #[serde(default, deserialize_with = "some_nullable")]
    telegram_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "some_nullable")]
    discord_url: Option<Option<String>>,
}

fn some_nullable<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

async fn view(pool: &PgPool, ws: Uuid) -> Result<ChannelsView, AppError> {
    let (t, d): (bool, bool) = sqlx::query_as(
        "SELECT coalesce(notify_telegram_webhook <> '', false), \
                coalesce(notify_discord_webhook <> '', false) \
         FROM workspaces WHERE id = $1",
    )
    .bind(ws)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("workspace not found"))?;
    Ok(ChannelsView {
        telegram_set: t,
        discord_set: d,
    })
}

/// Which notification channels are configured. URLs are never returned.
#[utoipa::path(
    get,
    path = "/api/v1/settings/notify-channels",
    tag = "settings",
    security(("cookie" = [])),
    responses((status = 200, body = ChannelsView), (status = 403, body = ErrorBody))
)]
pub async fn get_channels(
    State(s): State<AppState>,
    auth: Auth<Admin>,
) -> Result<Json<ChannelsView>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    Ok(Json(view(&s.pool, ws).await?))
}

async fn checked(url: &str) -> Result<(), AppError> {
    let ok_scheme = url.starts_with("https://") || url.starts_with("http://");
    if !ok_scheme || url.len() > MAX_URL {
        return Err(AppError::BadRequest(
            "url must be an absolute http(s) url".into(),
        ));
    }
    ssrf::check_url(url).await
}

/// Set or clear the Telegram and Discord webhook URLs (sealed at rest).
#[utoipa::path(
    put,
    path = "/api/v1/settings/notify-channels",
    tag = "settings",
    security(("cookie" = [])),
    request_body = PutChannels,
    responses((status = 200, body = ChannelsView), (status = 400, body = ErrorBody))
)]
pub async fn put_channels(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<PutChannels>,
) -> Result<Json<ChannelsView>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut sealed: Vec<(Channel, Option<String>)> = Vec::new();
    for (ch, v) in [
        (Channel::Telegram, req.telegram_url),
        (Channel::Discord, req.discord_url),
    ] {
        let Some(v) = v else { continue };
        let v = v.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        if let Some(u) = &v {
            checked(u).await?;
        }
        sealed.push((ch, v.map(|u| s.vault.seal_col(TABLE, ch.column(), ws, &u))));
    }
    let mut tx = s.pool.begin().await?;
    for (ch, value) in &sealed {
        // One static statement per column: the column name is never interpolated.
        let q = match ch {
            Channel::Telegram => sqlx::query(
                "UPDATE workspaces SET notify_telegram_webhook = $2, notify_enc_at = now() WHERE id = $1",
            ),
            Channel::Discord => sqlx::query(
                "UPDATE workspaces SET notify_discord_webhook = $2, notify_enc_at = now() WHERE id = $1",
            ),
        };
        let done = q.bind(ws).bind(value).execute(&mut *tx).await?;
        if done.rows_affected() == 0 {
            return Err(AppError::NotFound("workspace not found"));
        }
    }
    let changed: Vec<&str> = sealed.iter().map(|(c, _)| c.column()).collect();
    audit::record(
        &mut *tx,
        &auth.principal,
        "settings.notify_channels",
        "workspace",
        ws,
        Some(json!({ "channels": changed })),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(view(&s.pool, ws).await?))
}

#[derive(Deserialize, ToSchema)]
pub struct TestRequest {
    /// `telegram` or `discord`.
    channel: String,
}

#[derive(Serialize, ToSchema)]
pub struct TestResult {
    sent: bool,
}

/// Send a test message to a configured channel now, independent of the outbox.
#[utoipa::path(
    post,
    path = "/api/v1/settings/test-notify-channel",
    tag = "settings",
    security(("cookie" = [])),
    request_body = TestRequest,
    responses(
        (status = 200, body = TestResult),
        (status = 400, body = ErrorBody),
        (status = 422, body = ErrorBody),
    )
)]
pub async fn test_channel(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<TestRequest>,
) -> Result<Json<TestResult>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let ch = Channel::parse(&req.channel)
        .ok_or_else(|| AppError::BadRequest("channel must be telegram or discord".into()))?;
    let channels = Channels::new(crate::dispatch::http_client(), s.vault.clone());
    let url = channels
        .urls(&s.pool, ws)
        .await?
        .into_iter()
        .find(|(c, _)| *c == ch)
        .map(|(_, u)| u)
        .ok_or_else(|| AppError::Unprocessable("channel is not configured".into()))?;
    channels
        .send(ch, &url, "bunfiiliate: test message")
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "test notify failed");
            AppError::Unprocessable("the channel did not accept the message".into())
        })?;
    Ok(Json(TestResult { sent: true }))
}
