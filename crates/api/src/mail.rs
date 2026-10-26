//! Outgoing email (lettre) and `email_templates` CRUD with `{{var}}` substitution.
//!
//! `SMTP_URL` (lettre connection URL, e.g. `smtps://user:pass@host:465`) turns SMTP on;
//! unset means log-only. `SMTP_FROM` is the sender (default `bunfiiliate <noreply@localhost>`).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use bf_db::ops::EmailTemplate;
use lettre::message::Mailbox;
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::transport::stub::AsyncStubTransport;
use lettre::{AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Auth};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::page::{PageQuery, Paged};

const DEFAULT_FROM: &str = "bunfiiliate <noreply@localhost>";
const MAX_SUBJECT: usize = 200;
const MAX_BODY: usize = 20_000;

/// Template keys the dispatcher sends, with the built-in text used until a workspace
/// saves its own.
pub const TEMPLATES: [(&str, &str, &str); 2] = [
    (
        "conversion_approved",
        "Commission approved: {{commission}} {{currency}}",
        "Your commission of {{commission}} {{currency}} was approved.\n",
    ),
    (
        "payout_paid",
        "Payout sent: {{amount}} {{currency}}",
        "A payout of {{amount}} {{currency}} was sent to you.\n",
    ),
];

/// Template key for an outbox topic, if the topic sends mail.
pub fn template_key(topic: &str) -> Option<&'static str> {
    match topic {
        "conversion.approved" => Some("conversion_approved"),
        "payout.paid" => Some("payout_paid"),
        _ => None,
    }
}

pub fn default_template(key: &str) -> Option<(&'static str, &'static str)> {
    TEMPLATES.iter().find(|t| t.0 == key).map(|t| (t.1, t.2))
}

/// Replace `{{ name }}` with `vars[name]` (unknown names render empty). Values are not
/// scanned again, so a value containing `{{x}}` stays literal.
pub fn render(template: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        out.push_str(vars.get(after[..end].trim()).map_or("", String::as_str));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// `1234` -> `12.34`; integer math only.
fn fmt_cents(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// Template variables from an outbox payload: every scalar field by name, and each
/// `*_cents` integer also as a formatted amount without the suffix (`commission_cents`
/// -> `commission`).
pub fn vars(payload: &Value) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for (k, v) in payload.as_object().into_iter().flatten() {
        match v {
            Value::String(s) => vars.insert(k.clone(), s.clone()),
            Value::Number(n) => vars.insert(k.clone(), n.to_string()),
            _ => None,
        };
        if let (Some(name), Some(c)) = (k.strip_suffix("_cents"), v.as_i64()) {
            vars.insert(name.to_string(), fmt_cents(c));
        }
    }
    vars
}

#[derive(Clone)]
pub enum Mailer {
    /// No SMTP configured: messages are logged.
    Log,
    Smtp(Box<AsyncSmtpTransport<Tokio1Executor>>, Mailbox),
    /// Records messages in memory (tests).
    Stub(AsyncStubTransport, Mailbox),
}

impl Mailer {
    pub fn from_env() -> Result<Self, String> {
        let Ok(url) = std::env::var("SMTP_URL") else {
            return Ok(Self::Log);
        };
        let from = std::env::var("SMTP_FROM").unwrap_or_else(|_| DEFAULT_FROM.into());
        let from = from
            .parse()
            .map_err(|e| format!("invalid SMTP_FROM: {e}"))?;
        let smtp = AsyncSmtpTransport::<Tokio1Executor>::from_url(&url)
            .map_err(|e| format!("invalid SMTP_URL: {e}"))?
            .build();
        Ok(Self::Smtp(Box::new(smtp), from))
    }

    pub fn stub(transport: AsyncStubTransport) -> Self {
        let from = DEFAULT_FROM.parse().expect("default sender parses");
        Self::Stub(transport, from)
    }

    pub async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), String> {
        let (from, to) = match self {
            Self::Log => {
                tracing::info!(to, subject, "email (no SMTP configured)\n{body}");
                return Ok(());
            }
            Self::Smtp(_, from) | Self::Stub(_, from) => (from, to),
        };
        let msg = Message::builder()
            .from(from.clone())
            .to(to.parse().map_err(|e| format!("bad recipient: {e}"))?)
            .subject(subject)
            .body(body.to_string())
            .map_err(|e| e.to_string())?;
        match self {
            Self::Smtp(t, _) => t.send(msg).await.map(drop).map_err(|e| e.to_string()),
            Self::Stub(t, _) => t.send(msg).await.map(drop).map_err(|e| e.to_string()),
            Self::Log => Ok(()),
        }
    }
}

// ---- email-templates routes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/email-templates",
            get(list_templates).post(create_template),
        )
        .route(
            "/api/v1/email-templates/{id}",
            get(get_template)
                .patch(update_template)
                .delete(delete_template),
        )
}

fn not_found() -> AppError {
    AppError::NotFound("email template not found")
}

fn check_text(subject: Option<&str>, body: Option<&str>) -> Result<(), AppError> {
    if let Some(s) = subject
        && (s.trim().is_empty() || s.len() > MAX_SUBJECT || s.contains(['\r', '\n']))
    {
        return Err(AppError::BadRequest(
            "subject must be one line, 1-200 characters".into(),
        ));
    }
    if body.is_some_and(|b| b.len() > MAX_BODY) {
        return Err(AppError::BadRequest("body too long".into()));
    }
    Ok(())
}

#[derive(Deserialize, ToSchema)]
pub struct NewTemplate {
    /// `conversion_approved` or `payout_paid`.
    key: String,
    /// One line; `{{var}}` placeholders are substituted.
    subject: String,
    body: String,
}

/// Override the built-in text for one email. Variables: every scalar of the event payload
/// (`commission_cents`, `currency`, ...) plus the amount as `commission`/`amount`.
#[utoipa::path(
    post,
    path = "/api/v1/email-templates",
    tag = "email-templates",
    security(("cookie" = [])),
    request_body = NewTemplate,
    responses(
        (status = 201, body = EmailTemplate),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 409, description = "Key already has a template", body = ErrorBody),
    )
)]
pub async fn create_template(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<NewTemplate>,
) -> Result<(StatusCode, Json<EmailTemplate>), AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    if default_template(&req.key).is_none() {
        return Err(AppError::BadRequest(format!(
            "key must be one of: {}",
            TEMPLATES.map(|t| t.0).join(", ")
        )));
    }
    check_text(Some(&req.subject), Some(&req.body))?;
    let t = EmailTemplate {
        id: Uuid::now_v7(),
        workspace_id: ws,
        key: req.key,
        subject: req.subject,
        body: req.body,
    };
    let mut tx = s.pool.begin().await?;
    bf_db::ops::insert_template(&mut *tx, &t).await?;
    let diff = json!({ "key": t.key, "subject": t.subject });
    let action = "email_template.create";
    audit::record(
        &mut *tx,
        &auth.principal,
        action,
        "email_template",
        t.id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(t)))
}

/// List the workspace's template overrides.
#[utoipa::path(
    get,
    path = "/api/v1/email-templates",
    tag = "email-templates",
    security(("cookie" = [])),
    params(PageQuery),
    responses((status = 200, body = Paged<EmailTemplate>), (status = 403, body = ErrorBody))
)]
pub async fn list_templates(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Paged<EmailTemplate>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rows = bf_db::ops::list_templates(&s.pool, ws, page.limit(), page.offset()).await?;
    Ok(Json(page.wrap(rows)))
}

/// One template.
#[utoipa::path(
    get,
    path = "/api/v1/email-templates/{id}",
    tag = "email-templates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Template id")),
    responses((status = 200, body = EmailTemplate), (status = 404, body = ErrorBody))
)]
pub async fn get_template(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<EmailTemplate>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let t = bf_db::ops::get_template(&s.pool, ws, id).await?;
    Ok(Json(t.ok_or_else(not_found)?))
}

#[derive(Deserialize, ToSchema)]
pub struct TemplatePatch {
    subject: Option<String>,
    body: Option<String>,
}

/// Change a template's subject and/or body.
#[utoipa::path(
    patch,
    path = "/api/v1/email-templates/{id}",
    tag = "email-templates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Template id")),
    request_body = TemplatePatch,
    responses(
        (status = 200, body = EmailTemplate),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
pub async fn update_template(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
    AppJson(req): AppJson<TemplatePatch>,
) -> Result<Json<EmailTemplate>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    check_text(req.subject.as_deref(), req.body.as_deref())?;
    let mut tx = s.pool.begin().await?;
    let t = bf_db::ops::update_template(
        &mut *tx,
        ws,
        id,
        req.subject.as_deref(),
        req.body.as_deref(),
    )
    .await?
    .ok_or_else(not_found)?;
    let diff = json!({ "subject": req.subject, "body_changed": req.body.is_some() });
    let action = "email_template.update";
    audit::record(
        &mut *tx,
        &auth.principal,
        action,
        "email_template",
        id,
        Some(diff),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(t))
}

/// Delete an override; the built-in text applies again.
#[utoipa::path(
    delete,
    path = "/api/v1/email-templates/{id}",
    tag = "email-templates",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Template id")),
    responses((status = 204), (status = 404, body = ErrorBody))
)]
pub async fn delete_template(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let mut tx = s.pool.begin().await?;
    if !bf_db::ops::delete_template(&mut *tx, ws, id).await? {
        return Err(not_found());
    }
    let action = "email_template.delete";
    audit::record(
        &mut *tx,
        &auth.principal,
        action,
        "email_template",
        id,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn ops_render_substitutes_and_leaves_no_placeholders() {
        let v = vars_of(&[("a", "1"), ("b", "{{a}}")]);
        assert_eq!(
            render("x {{a}} y {{ b }} z {{nope}}.", &v),
            "x 1 y {{a}} z ."
        );
        assert_eq!(render("open {{a", &v), "open {{a");
        assert_eq!(render("", &v), "");
        assert_eq!(render("{{a}}{{a}}", &v), "11");
    }

    #[test]
    fn ops_vars_formats_cents_without_floats() {
        let v = vars(
            &json!({ "commission_cents": 1205, "currency": "usd", "n": 3, "ok": true,
                              "neg_cents": -5, "big_cents": i64::MIN }),
        );
        assert_eq!(v["commission"], "12.05");
        assert_eq!(v["commission_cents"], "1205");
        assert_eq!(v["currency"], "usd");
        assert_eq!(v["neg"], "-0.05");
        assert_eq!(v["big"], "-92233720368547758.08");
        assert!(!v.contains_key("ok"));
    }
}
