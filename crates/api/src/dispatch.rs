//! Outbox dispatcher. Two worker ticks:
//!
//! - [`dispatch_outbox`] claims each outbox row once (`FOR UPDATE SKIP LOCKED`) and, in one
//!   transaction, queues a `webhook_deliveries` row per matching active endpoint of the
//!   program's workspace and writes the affiliate's notification. The row is then done; the
//!   email goes out right after the commit (best effort: a failure is logged, not retried).
//! - [`deliver_due`] leases due deliveries, then sends them concurrently. A non-2xx answer or a
//!   network error schedules the next attempt with exponential backoff; after [`MAX_ATTEMPTS`]
//!   the delivery is dead (`next_attempt_at` null).

use std::time::Duration;

use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::mail::{Mailer, default_template, render, template_key, vars};
use crate::metrics;
use crate::ssrf;

pub const MAX_ATTEMPTS: i32 = 8;
const BACKOFF_BASE_SECS: i32 = 30;
const BACKOFF_CAP_SECS: i32 = 3600;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Delay before the attempt that follows failed attempt number `attempt` (1-based):
/// 30s, 60s, 120s, ... capped at one hour.
pub fn backoff_secs(attempt: i32) -> i32 {
    let exp = u32::try_from(attempt.saturating_sub(1))
        .unwrap_or(0)
        .min(16);
    BACKOFF_BASE_SECS
        .saturating_mul(1 << exp)
        .min(BACKOFF_CAP_SECS)
}

/// `X-Bf-Signature` value: `t=<unix>,v1=<hex HMAC-SHA256 of "<t>.<body>">`.
pub fn signature(secret: &str, ts: i64, body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(format!("{ts}.{body}").as_bytes());
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("t={ts},v1={hex}")
}

/// HTTP client for deliveries: bounded time, never follows redirects.
///
/// Unless `BF_ALLOW_PRIVATE_WEBHOOKS` is set it only connects to public addresses (see [`ssrf`]).
pub fn http_client() -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    let builder = if ssrf::allow_private() {
        builder
    } else {
        builder.dns_resolver(ssrf::PublicOnly)
    };
    builder.build().expect("static client config")
}

fn uuid_field(payload: &Value, key: &str) -> Option<Uuid> {
    payload.get(key)?.as_str()?.parse().ok()
}

/// Dispatch every ready outbox row; returns how many were claimed.
pub async fn dispatch_outbox(pool: &PgPool, mailer: &Mailer) -> Result<u64, sqlx::Error> {
    let mut n = 0;
    while dispatch_next(pool, mailer).await? {
        n += 1;
    }
    Ok(n)
}

struct Mail {
    to: String,
    subject: String,
    body: String,
}

async fn dispatch_next(pool: &PgPool, mailer: &Mailer) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let Some((id, topic, payload)) = sqlx::query_as::<_, (i64, String, Value)>(
        "SELECT id, topic, payload FROM outbox \
         WHERE done_at IS NULL AND available_at <= now() \
         ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };

    let mut mail = None;
    if let Some(program) = uuid_field(&payload, "program_id") {
        queue_deliveries(&mut tx, id, &topic, program).await?;
        mail = notify(&mut tx, &topic, &payload, program).await?;
    }
    sqlx::query("UPDATE outbox SET done_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    if let Some(m) = mail
        && let Err(e) = mailer.send(&m.to, &m.subject, &m.body).await
    {
        tracing::warn!(outbox = id, error = %e, "email failed");
    }
    Ok(true)
}

/// One pending delivery per active endpoint of the program's workspace that wants `topic`.
async fn queue_deliveries(
    c: &mut PgConnection,
    outbox: i64,
    topic: &str,
    program: Uuid,
) -> Result<(), sqlx::Error> {
    let endpoints: Vec<Uuid> = sqlx::query_scalar(
        "SELECT e.id FROM webhook_endpoints e JOIN programs p ON p.workspace_id = e.workspace_id \
         WHERE p.id = $1 AND e.active AND (cardinality(e.events) = 0 OR $2 = ANY(e.events))",
    )
    .bind(program)
    .bind(topic)
    .fetch_all(&mut *c)
    .await?;
    for endpoint in endpoints {
        sqlx::query(
            "INSERT INTO webhook_deliveries (id, endpoint_id, outbox_id, attempt, next_attempt_at) \
             VALUES ($1, $2, $3, 0, now())",
        )
        .bind(Uuid::now_v7())
        .bind(endpoint)
        .bind(outbox)
        .execute(&mut *c)
        .await?;
    }
    Ok(())
}

/// Notification row plus the email to send, for topics that have a template and an
/// affiliate (`payload.affiliate_id`) in the program's workspace.
async fn notify(
    c: &mut PgConnection,
    topic: &str,
    payload: &Value,
    program: Uuid,
) -> Result<Option<Mail>, sqlx::Error> {
    let (Some(key), Some(affiliate)) = (template_key(topic), uuid_field(payload, "affiliate_id"))
    else {
        return Ok(None);
    };
    let Some((workspace, user, email)) = sqlx::query_as::<_, (Uuid, Uuid, String)>(
        "SELECT p.workspace_id, u.id, u.email::text FROM programs p \
         JOIN affiliates a ON a.id = $2 AND a.workspace_id = p.workspace_id \
         JOIN users u ON u.id = a.user_id WHERE p.id = $1",
    )
    .bind(program)
    .bind(affiliate)
    .fetch_optional(&mut *c)
    .await?
    else {
        return Ok(None);
    };
    let (subject, body) = match bf_db::ops::template_by_key(&mut *c, workspace, key).await? {
        Some(t) => (t.subject, t.body),
        None => match default_template(key) {
            Some((s, b)) => (s.to_string(), b.to_string()),
            None => return Ok(None),
        },
    };
    let vars = vars(payload);
    let (subject, body) = (render(&subject, &vars), render(&body, &vars));
    sqlx::query("INSERT INTO notifications (id, user_id, kind, body) VALUES ($1, $2, $3, $4)")
        .bind(Uuid::now_v7())
        .bind(user)
        .bind(topic)
        .bind(&subject)
        .execute(&mut *c)
        .await?;
    Ok(Some(Mail {
        to: email,
        subject,
        body,
    }))
}

/// Deliveries leased per tick, and sent at once. The rest wait for the next tick.
const DELIVER_PER_TICK: i64 = 200;
const DELIVER_CONCURRENCY: usize = 16;
/// How long a leased delivery stays invisible to other ticks while its request is in flight.
/// If the process dies mid-send the delivery is retried after this.
const LEASE_SECS: i32 = 60;

/// Send due deliveries once each; returns how many were attempted.
///
/// Rows are leased (their `next_attempt_at` pushed out and committed) before any HTTP call, so
/// no transaction or row lock is held while a slow endpoint answers, and a slow endpoint only
/// holds up its own request, not the worker's other ticks.
pub async fn deliver_due(pool: &PgPool, http: &reqwest::Client) -> Result<u64, sqlx::Error> {
    let due = lease_due(pool).await?;
    let attempted = due.len() as u64;
    let mut due = due.into_iter();
    loop {
        let mut sends = tokio::task::JoinSet::new();
        for d in due.by_ref().take(DELIVER_CONCURRENCY) {
            let (pool, http) = (pool.clone(), http.clone());
            sends.spawn(async move { send_one(&pool, &http, d).await });
        }
        if sends.is_empty() {
            return Ok(attempted);
        }
        while let Some(res) = sends.join_next().await {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::error!(error = %e, "recording delivery failed"),
                Err(e) => tracing::error!(error = %e, "delivery task failed"),
            }
        }
    }
}

#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    attempt: i32,
    url: String,
    secret: String,
    outbox_id: i64,
    topic: String,
    payload: Value,
}

/// Claim up to [`DELIVER_PER_TICK`] due deliveries by pushing their next attempt [`LEASE_SECS`]
/// out.
async fn lease_due(pool: &PgPool) -> Result<Vec<Due>, sqlx::Error> {
    sqlx::query_as::<_, Due>(
        "WITH due AS ( \
             SELECT d.id FROM webhook_deliveries d \
             JOIN webhook_endpoints e ON e.id = d.endpoint_id AND e.active \
             WHERE d.next_attempt_at <= now() \
             ORDER BY d.next_attempt_at, d.id LIMIT $1 FOR UPDATE OF d SKIP LOCKED), \
         leased AS ( \
             UPDATE webhook_deliveries d SET next_attempt_at = now() + $2 * interval '1 second' \
             FROM due WHERE d.id = due.id RETURNING d.id, d.attempt, d.endpoint_id, d.outbox_id) \
         SELECT l.id, l.attempt, e.url, e.secret, o.id AS outbox_id, o.topic, o.payload \
         FROM leased l JOIN webhook_endpoints e ON e.id = l.endpoint_id \
         JOIN outbox o ON o.id = l.outbox_id",
    )
    .bind(DELIVER_PER_TICK)
    .bind(LEASE_SECS)
    .fetch_all(pool)
    .await
}

/// POST one leased delivery and record the outcome.
async fn send_one(pool: &PgPool, http: &reqwest::Client, d: Due) -> Result<(), sqlx::Error> {
    let body = json!({ "id": d.outbox_id, "topic": d.topic, "data": d.payload }).to_string();
    let ts = Utc::now().timestamp();
    let sent = match ssrf::check_literal_host(&d.url) {
        Err(_) => Err("url points at a private address".to_string()),
        Ok(()) => http
            .post(&d.url)
            .header("content-type", "application/json")
            .header("x-bf-event", &d.topic)
            .header("x-bf-delivery", d.id.to_string())
            .header("x-bf-signature", signature(&d.secret, ts, &body))
            .body(body)
            .send()
            .await
            .map_err(|e| e.to_string()),
    };
    let (status, delivered) = match &sent {
        Ok(r) => (
            Some(i32::from(r.status().as_u16())),
            r.status().is_success(),
        ),
        Err(e) => {
            tracing::warn!(delivery = %d.id, error = %e, "webhook send failed");
            (None, false)
        }
    };
    if !delivered {
        metrics::WEBHOOK_FAILURES.inc();
    }
    let attempt = d.attempt + 1;
    let retry_in = (!delivered && attempt < MAX_ATTEMPTS).then(|| backoff_secs(attempt));
    sqlx::query(
        "UPDATE webhook_deliveries SET attempt = $2, status_code = $3, \
         next_attempt_at = now() + $4 * interval '1 second' WHERE id = $1",
    )
    .bind(d.id)
    .bind(attempt)
    .bind(status)
    .bind(retry_in)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_backoff_doubles_then_caps() {
        let secs: Vec<_> = (1..=9).map(backoff_secs).collect();
        assert_eq!(secs, [30, 60, 120, 240, 480, 960, 1920, 3600, 3600]);
        assert_eq!(backoff_secs(i32::MAX), 3600);
    }

    #[test]
    fn ops_signature_known_vector() {
        // HMAC-SHA256(key="secret", "1700000000.{}") computed independently with Python.
        assert_eq!(
            signature("secret", 1_700_000_000, "{}"),
            "t=1700000000,v1=b8569b78799ff9e3cbff0fc2d63a33a2b57f3282abd07c37ae5e8e7d79a5f163"
        );
    }
}
