//! Payout rails: async provider adapters, sealed per-workspace credentials, and the dispatch /
//! reconcile flow.
//!
//! Money safety: no provider call runs inside a DB transaction. A line is marked `submitted`
//! (committed) first, the provider is called with the line id as idempotency key, and only a
//! provider-confirmed `paid` posts the ledger, in its own short transaction. A crash or timeout
//! leaves the line `submitted`; retrying re-sends the same key, so the provider dedupes it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::routing::{get, post as post_route, put};
use bf_core::{
    LineEvent, LineState, Money, PayoutDestination, PayoutError, PayoutEvent, PayoutReceipt,
    PayoutStatus, Rail, RailStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::audit;
use crate::auth::{Admin, Auth};
use crate::error::{AppError, AppJson, ErrorBody};
use crate::payouts::{self, BatchDetail};

// ---------------------------------------------------------------------------------------------
// Adapters
// ---------------------------------------------------------------------------------------------

/// In-process provider for tests and for the `mock` rail. Deduplicates by idempotency key like
/// a real provider; behaviour is scripted per destination email.
#[derive(Default)]
pub struct MockRail {
    state: Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    probe: Option<PgPool>,
    idle_in_tx: i64,
    reject: HashSet<String>,
    transient: HashSet<String>,
    hold: HashSet<String>,
    released: HashSet<Uuid>,
    sent: HashMap<Uuid, i64>,
    calls: u32,
}

impl MockRail {
    /// Before each provider call, count this database's connections sitting in an open
    /// transaction (see [`MockRail::idle_in_tx_seen`]): a provider call inside a DB
    /// transaction would show up there.
    pub fn probe(&self, pool: PgPool) {
        self.lock().probe = Some(pool);
    }
    /// Most open-transaction connections seen at the moment of a provider call.
    pub fn idle_in_tx_seen(&self) -> i64 {
        self.lock().idle_in_tx
    }
    /// Refuse payouts to this email.
    pub fn reject(&self, email: &str) {
        self.lock().reject.insert(email.into());
    }
    /// Time out (unknown outcome) for this email.
    pub fn transient(&self, email: &str) {
        self.lock().transient.insert(email.into());
    }
    /// Accept but report `pending` until [`MockRail::release`].
    pub fn hold(&self, email: &str) {
        self.lock().hold.insert(email.into());
    }
    /// Clear scripted failures for this email.
    pub fn heal(&self, email: &str) {
        let mut s = self.lock();
        s.reject.remove(email);
        s.transient.remove(email);
    }
    /// Let held payouts settle.
    pub fn release(&self) {
        self.lock().hold.clear();
    }
    /// Distinct idempotency keys the provider accepted.
    pub fn accepted(&self) -> usize {
        self.lock().sent.len()
    }
    /// Total cents the provider would have moved (one per key).
    pub fn total_cents(&self) -> i64 {
        self.lock().sent.values().sum()
    }
    /// Provider calls made, including repeats.
    pub fn calls(&self) -> u32 {
        self.lock().calls
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, MockState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn payout(
        &self,
        key: Uuid,
        dest: &PayoutDestination,
        amount: Money,
    ) -> Result<(PayoutReceipt, RailStatus), PayoutError> {
        let mut s = self.lock();
        s.calls += 1;
        if s.reject.contains(&dest.email) {
            return Err(PayoutError::Rejected("mock: recipient refused".into()));
        }
        if s.transient.contains(&dest.email) {
            return Err(PayoutError::Transient("mock: timeout".into()));
        }
        s.sent.entry(key).or_insert(amount.cents);
        let status = if s.hold.contains(&dest.email) {
            RailStatus::Pending
        } else {
            RailStatus::Paid
        };
        Ok((
            PayoutReceipt {
                reference: format!("mock-{key}"),
            },
            status,
        ))
    }

    fn status(&self, receipt: &PayoutReceipt) -> Result<RailStatus, PayoutError> {
        let s = self.lock();
        let key = receipt
            .reference
            .strip_prefix("mock-")
            .and_then(|k| k.parse::<Uuid>().ok())
            .ok_or_else(|| PayoutError::Rejected("mock: unknown receipt".into()))?;
        if !s.sent.contains_key(&key) {
            return Err(PayoutError::Rejected("mock: unknown receipt".into()));
        }
        Ok(if s.released.contains(&key) || s.hold.is_empty() {
            RailStatus::Paid
        } else {
            RailStatus::Pending
        })
    }
}

/// Currencies without minor units; the cents-based amount math below does not apply.
const ZERO_DECIMAL: &[&str] = &["JPY", "KRW", "VND", "CLP", "ISK", "HUF", "TWD"];

fn amount_text(m: Money) -> Result<String, PayoutError> {
    if ZERO_DECIMAL.contains(&m.currency.as_str()) || m.cents <= 0 {
        return Err(PayoutError::Rejected(
            "unsupported amount or currency".into(),
        ));
    }
    Ok(format!("{}.{:02}", m.cents / 100, m.cents % 100))
}

/// Send one request; sort failures into "refused" (4xx) and "unknown outcome" (network, 5xx).
async fn call(req: reqwest::RequestBuilder) -> Result<Value, PayoutError> {
    let resp = req
        .send()
        .await
        .map_err(|e| PayoutError::Transient(format!("request failed: {}", e.without_url())))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let detail: String = text.chars().take(300).collect();
    if status.is_server_error() || status.as_u16() == 429 {
        Err(PayoutError::Transient(format!("http {status}: {detail}")))
    } else {
        Err(PayoutError::Rejected(format!("http {status}: {detail}")))
    }
}

#[derive(Clone)]
pub struct PaypalClient {
    http: reqwest::Client,
    base: &'static str,
    client_id: String,
    client_secret: String,
}

impl PaypalClient {
    fn new(http: reqwest::Client, sandbox: bool, c: &HashMap<String, String>) -> Option<Self> {
        Some(Self {
            http,
            base: if sandbox {
                "https://api-m.sandbox.paypal.com"
            } else {
                "https://api-m.paypal.com"
            },
            client_id: c.get("client_id")?.clone(),
            client_secret: c.get("client_secret")?.clone(),
        })
    }

    async fn token(&self) -> Result<String, PayoutError> {
        let v = call(
            self.http
                .post(format!("{}/v1/oauth2/token", self.base))
                .basic_auth(&self.client_id, Some(&self.client_secret))
                .header("content-type", "application/x-www-form-urlencoded")
                .body("grant_type=client_credentials"),
        )
        .await?;
        v["access_token"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| PayoutError::Rejected("paypal: no access token".into()))
    }

    async fn payout(
        &self,
        key: Uuid,
        dest: &PayoutDestination,
        amount: Money,
    ) -> Result<(PayoutReceipt, RailStatus), PayoutError> {
        let value = amount_text(amount)?;
        let token = self.token().await?;
        let body = json!({
            "sender_batch_header": {
                "sender_batch_id": key.to_string(),
                "email_subject": "Your affiliate payout",
            },
            "items": [{
                "recipient_type": "EMAIL",
                "receiver": dest.email,
                "sender_item_id": key.to_string(),
                "amount": {"value": value, "currency": amount.currency.as_str()},
            }],
        });
        let v = call(
            self.http
                .post(format!("{}/v1/payments/payouts", self.base))
                .bearer_auth(token)
                .header("content-type", "application/json")
                .header("PayPal-Request-Id", key.to_string())
                .body(body.to_string()),
        )
        .await?;
        let id = v["batch_header"]["payout_batch_id"]
            .as_str()
            .ok_or_else(|| PayoutError::Transient("paypal: no batch id".into()))?;
        let st = paypal_batch_status(v["batch_header"]["batch_status"].as_str().unwrap_or(""));
        Ok((
            PayoutReceipt {
                reference: id.into(),
            },
            st,
        ))
    }

    async fn status(&self, r: &PayoutReceipt) -> Result<RailStatus, PayoutError> {
        let token = self.token().await?;
        let v = call(
            self.http
                .get(format!("{}/v1/payments/payouts/{}", self.base, r.reference))
                .bearer_auth(token),
        )
        .await?;
        // One item per batch: the item status is the one that says whether money moved.
        if let Some(item) = v["items"].get(0) {
            return Ok(paypal_item_status(
                item["transaction_status"].as_str().unwrap_or(""),
            ));
        }
        Ok(paypal_batch_status(
            v["batch_header"]["batch_status"].as_str().unwrap_or(""),
        ))
    }
}

/// PayPal reports `SUCCESS` for a batch only once its items settled; anything else is pending
/// unless the batch itself was refused.
pub fn paypal_batch_status(s: &str) -> RailStatus {
    match s {
        "SUCCESS" => RailStatus::Paid,
        "DENIED" | "CANCELED" => RailStatus::Failed,
        _ => RailStatus::Pending,
    }
}

pub fn paypal_item_status(s: &str) -> RailStatus {
    match s {
        "SUCCESS" => RailStatus::Paid,
        "FAILED" | "DENIED" | "RETURNED" | "REFUNDED" | "REVERSED" | "BLOCKED" => {
            RailStatus::Failed
        }
        _ => RailStatus::Pending,
    }
}

#[derive(Clone)]
pub struct WiseClient {
    http: reqwest::Client,
    base: &'static str,
    token: String,
    profile: String,
}

impl WiseClient {
    fn new(http: reqwest::Client, sandbox: bool, c: &HashMap<String, String>) -> Option<Self> {
        Some(Self {
            http,
            base: if sandbox {
                "https://api.sandbox.transferwise.tech"
            } else {
                "https://api.wise.com"
            },
            token: c.get("api_token")?.clone(),
            profile: c.get("profile_id")?.clone(),
        })
    }

    fn req(&self, m: reqwest::Method, path: &str, body: Option<Value>) -> reqwest::RequestBuilder {
        let r = self
            .http
            .request(m, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        match body {
            Some(b) => r
                .header("content-type", "application/json")
                .body(b.to_string()),
            None => r,
        }
    }

    /// Quote -> email recipient -> transfer (`customerTransactionId` = idempotency key) -> fund
    /// from balance. Only the transfer is idempotent at Wise; quote and recipient are harmless
    /// to repeat. Not exercised against the Wise sandbox in CI.
    async fn payout(
        &self,
        key: Uuid,
        dest: &PayoutDestination,
        amount: Money,
    ) -> Result<(PayoutReceipt, RailStatus), PayoutError> {
        amount_text(amount)?;
        let cur = amount.currency.as_str();
        let value = amount.cents as f64 / 100.0;
        let quote = call(self.req(
            reqwest::Method::POST,
            &format!("/v3/profiles/{}/quotes", self.profile),
            Some(json!({"sourceCurrency": cur, "targetCurrency": cur, "targetAmount": value})),
        ))
        .await?;
        let quote_id = quote["id"]
            .as_str()
            .ok_or_else(|| PayoutError::Rejected("wise: no quote id".into()))?;
        let acct = call(self.req(
            reqwest::Method::POST,
            "/v1/accounts",
            Some(json!({
                "profile": self.profile.parse::<i64>().unwrap_or_default(),
                "currency": cur, "type": "email",
                "accountHolderName": dest.email,
                "details": {"email": dest.email},
            })),
        ))
        .await?;
        let target = acct["id"]
            .as_i64()
            .ok_or_else(|| PayoutError::Rejected("wise: no recipient id".into()))?;
        let t = call(self.req(
            reqwest::Method::POST,
            "/v1/transfers",
            Some(json!({
                "targetAccount": target, "quoteUuid": quote_id,
                "customerTransactionId": key.to_string(),
                "details": {"reference": "affiliate payout"},
            })),
        ))
        .await?;
        let id = t["id"]
            .as_i64()
            .ok_or_else(|| PayoutError::Transient("wise: no transfer id".into()))?;
        call(self.req(
            reqwest::Method::POST,
            &format!("/v3/profiles/{}/transfers/{id}/payments", self.profile),
            Some(json!({"type": "BALANCE"})),
        ))
        .await?;
        Ok((
            PayoutReceipt {
                reference: id.to_string(),
            },
            RailStatus::Pending,
        ))
    }

    async fn status(&self, r: &PayoutReceipt) -> Result<RailStatus, PayoutError> {
        let v = call(self.req(
            reqwest::Method::GET,
            &format!("/v1/transfers/{}", r.reference),
            None,
        ))
        .await?;
        Ok(wise_status(v["status"].as_str().unwrap_or("")))
    }
}

pub fn wise_status(s: &str) -> RailStatus {
    match s {
        "outgoing_payment_sent" => RailStatus::Paid,
        "cancelled" | "funds_refunded" | "bounced_back" | "charged_back" => RailStatus::Failed,
        _ => RailStatus::Pending,
    }
}

/// A runtime-selected provider. An enum, not `dyn`, like `Source`.
#[derive(Clone)]
pub enum RailClient {
    Mock(Arc<MockRail>),
    Paypal(PaypalClient),
    Wise(WiseClient),
}

impl RailClient {
    pub async fn payout(
        &self,
        key: Uuid,
        dest: &PayoutDestination,
        amount: Money,
    ) -> Result<(PayoutReceipt, RailStatus), PayoutError> {
        match self {
            RailClient::Mock(m) => {
                let probe = m.lock().probe.clone();
                if let Some(pool) = probe {
                    let n: i64 = sqlx::query_scalar(
                        "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() \
                         AND state LIKE 'idle in transaction%'",
                    )
                    .fetch_one(&pool)
                    .await
                    .unwrap_or(0);
                    let mut s = m.lock();
                    s.idle_in_tx = s.idle_in_tx.max(n);
                }
                m.payout(key, dest, amount)
            }
            RailClient::Paypal(p) => p.payout(key, dest, amount).await,
            RailClient::Wise(w) => w.payout(key, dest, amount).await,
        }
    }

    pub async fn status(&self, r: &PayoutReceipt) -> Result<RailStatus, PayoutError> {
        match self {
            RailClient::Mock(m) => m.status(r),
            RailClient::Paypal(p) => p.status(r).await,
            RailClient::Wise(w) => w.status(r).await,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Credentials (sealed)
// ---------------------------------------------------------------------------------------------

const CRED_TABLE: &str = "payout_rail_credentials";

fn cred_keys(rail: Rail) -> &'static [&'static str] {
    match rail {
        Rail::PaypalPayouts => &["client_id", "client_secret"],
        Rail::Wise => &["api_token", "profile_id"],
        _ => &[],
    }
}

/// Build the provider for `rail` from the workspace's sealed credentials. `422` when a rail
/// that needs credentials has none; `mock` is always available; `manual_csv` is not a provider.
pub async fn load_client(
    pool: &PgPool,
    vault: &bf_core::secrets::Vault,
    ws: Uuid,
    rail: Rail,
) -> Result<RailClient, AppError> {
    match rail {
        Rail::ManualCsv => Err(AppError::BadRequest("manual_csv has no provider".into())),
        Rail::Mock => Ok(RailClient::Mock(Arc::default())),
        Rail::PaypalPayouts | Rail::Wise => {
            let row: Option<(Uuid, bool, String)> = sqlx::query_as(
                "SELECT id, sandbox, creds FROM payout_rail_credentials \
                 WHERE workspace_id = $1 AND rail = $2",
            )
            .bind(ws)
            .bind(rail.as_str())
            .fetch_optional(pool)
            .await?;
            let (id, sandbox, stored) = row.ok_or(AppError::Unprocessable(
                "payout rail is not configured".into(),
            ))?;
            let plain = vault
                .open_col(CRED_TABLE, "creds", id, &stored)
                .map_err(|e| {
                    tracing::error!(error = %e, "cannot open payout credentials");
                    AppError::Internal
                })?;
            let creds: HashMap<String, String> =
                serde_json::from_str(&plain).map_err(|_| AppError::Internal)?;
            let http = crate::dispatch::http_client();
            let client = match rail {
                Rail::PaypalPayouts => {
                    PaypalClient::new(http, sandbox, &creds).map(RailClient::Paypal)
                }
                _ => WiseClient::new(http, sandbox, &creds).map(RailClient::Wise),
            };
            client.ok_or(AppError::Unprocessable(
                "payout rail credentials are incomplete".into(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Dispatch and reconcile
// ---------------------------------------------------------------------------------------------

/// What a dispatch or reconcile pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct Outcome {
    /// Lines confirmed paid (ledger posted) in this pass.
    pub paid: u32,
    /// Lines left at the provider, not yet confirmed.
    pub submitted: u32,
    /// Lines the provider refused.
    pub failed: u32,
    /// The whole batch is now paid.
    pub batch_paid: bool,
}

struct Line {
    id: Uuid,
    email: String,
    state: Option<LineState>,
    receipt: Option<String>,
}

fn reference(v: &Value) -> Option<String> {
    v.get("reference")?.as_str().map(str::to_owned)
}

async fn lines(pool: &PgPool, ws: Uuid, batch: Uuid) -> Result<Vec<Line>, AppError> {
    let rows: Vec<(Uuid, String, Option<String>, Option<Value>)> = sqlx::query_as(
        "SELECT p.id, COALESCE(p.payout_email, a.payout_email, u.email)::text, \
                p.rail_state, p.rail_receipt \
         FROM payouts p JOIN payout_batches b ON b.id = p.batch_id \
           JOIN programs pr ON pr.id = b.program_id \
           JOIN affiliates a ON a.id = p.affiliate_id JOIN users u ON u.id = a.user_id \
         WHERE p.batch_id = $1 AND pr.workspace_id = $2 AND p.status = 'approved' \
           AND p.amount_cents > 0 ORDER BY p.id",
    )
    .bind(batch)
    .bind(ws)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, email, st, receipt)| Line {
            id,
            email,
            state: st.as_deref().and_then(LineState::parse),
            receipt: receipt.as_ref().and_then(reference),
        })
        .collect())
}

/// Record a provider answer on a line still awaiting confirmation.
async fn record(
    pool: &PgPool,
    ws: Uuid,
    line: Uuid,
    receipt: Option<&str>,
    state: Option<LineState>,
    status: Option<&str>,
    error: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE payouts p SET rail_receipt = COALESCE($3, p.rail_receipt), \
           rail_state = COALESCE($4, p.rail_state), rail_status = COALESCE($5, p.rail_status), \
           rail_error = $6 \
         FROM payout_batches b JOIN programs pr ON pr.id = b.program_id \
         WHERE p.id = $1 AND p.batch_id = b.id AND pr.workspace_id = $2 \
           AND p.status = 'approved' AND p.rail_state IN ('pending', 'submitted', 'failed')",
    )
    .bind(line)
    .bind(ws)
    .bind(receipt.map(|r| json!({"reference": r})))
    .bind(state.map(LineState::as_str))
    .bind(status)
    .bind(error.map(|e| e.chars().take(300).collect::<String>()))
    .execute(pool)
    .await?;
    Ok(())
}

/// Post the ledger for a line the provider confirmed paid; one short transaction, no IO.
/// Idempotent: a line already paid is left alone.
async fn settle_confirmed(
    pool: &PgPool,
    ws: Uuid,
    batch: Uuid,
    line: Uuid,
    receipt: Option<&str>,
) -> Result<bool, AppError> {
    let mut tx = pool.begin().await?;
    let b = payouts::lock(&mut tx, ws, batch).await?;
    let row: Option<(Uuid, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT affiliate_id, amount_cents, status::text, rail_state FROM payouts \
         WHERE id = $1 AND batch_id = $2 FOR UPDATE", // scope: batch locked with the workspace predicate by lock()
    )
    .bind(line)
    .bind(batch)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((affiliate, cents, st, rs)) = row else {
        return Ok(false);
    };
    let ok = payouts::status(&st) == Some(PayoutStatus::Approved)
        && rs
            .as_deref()
            .and_then(LineState::parse)
            .is_some_and(|s| s.transition(LineEvent::Confirm).is_ok());
    if !ok {
        return Ok(false);
    }
    if let Some(r) = receipt {
        // scope: payout line locked above through its workspace-scoped batch
        sqlx::query("UPDATE payouts SET rail_receipt = $2 WHERE id = $1")
            .bind(line)
            .bind(json!({"reference": r}))
            .execute(&mut *tx)
            .await?;
    }
    let program = payouts::batch_program(&mut tx, batch).await?;
    payouts::settle_line(&mut tx, b.currency, program, (line, affiliate), cents, true).await?;
    tx.commit().await?;
    Ok(true)
}

/// Close out lines with nothing to send, then the batch once every line is paid.
async fn close_batch_if_done(pool: &PgPool, ws: Uuid, batch: Uuid) -> Result<bool, AppError> {
    let mut tx = pool.begin().await?;
    let b = payouts::lock(&mut tx, ws, batch).await?;
    if b.status != PayoutStatus::Approved {
        return Ok(b.status == PayoutStatus::Paid);
    }
    // scope: payout batch already locked with the workspace predicate by lock()
    let open: Vec<(Uuid, Uuid, i64)> = sqlx::query_as(
        "SELECT id, affiliate_id, amount_cents FROM payouts \
         WHERE batch_id = $1 AND status = 'approved' ORDER BY id FOR UPDATE",
    )
    .bind(batch)
    .fetch_all(&mut *tx)
    .await?;
    if open.iter().any(|o| o.2 > 0) {
        return Ok(false);
    }
    let program = payouts::batch_program(&mut tx, batch).await?;
    for (id, affiliate, cents) in open {
        payouts::settle_line(&mut tx, b.currency, program, (id, affiliate), cents, false).await?;
    }
    payouts::finalize_batch(&mut tx, batch).await?;
    tx.commit().await?;
    Ok(true)
}

/// Send every unsent or failed line of an approved batch through `client`. Lines the provider
/// already holds (submitted with a receipt) are left for [`reconcile`]. A provider failure on one
/// line never blocks the others; call again to retry failures (same key, no double pay).
pub async fn dispatch(
    pool: &PgPool,
    ws: Uuid,
    batch: Uuid,
    rail: Rail,
    client: &RailClient,
) -> Result<Outcome, AppError> {
    let currency = {
        let mut tx = pool.begin().await?;
        let b = payouts::lock(&mut tx, ws, batch).await?;
        b.status
            .transition(PayoutEvent::MarkPaid(Money::new(0, b.currency)))
            .map_err(payouts::invalid)?;
        // A line in flight on another rail must be settled there first.
        // scope: payout batch already locked with the workspace predicate by lock()
        let other: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM payouts WHERE batch_id = $1 AND status = 'approved' \
             AND rail <> $2 AND rail_state IN ('pending', 'submitted')",
        )
        .bind(batch)
        .bind(rail.as_str())
        .fetch_one(&mut *tx)
        .await?;
        if other > 0 {
            return Err(AppError::Conflict("a payout is in flight on another rail"));
        }
        // scope: payout batch already locked with the workspace predicate by lock()
        sqlx::query(
            "UPDATE payouts SET rail = $2, rail_state = 'pending' \
             WHERE batch_id = $1 AND status = 'approved' AND amount_cents > 0 \
               AND rail_state IS NULL",
        )
        .bind(batch)
        .bind(rail.as_str())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        b.currency
    };

    let mut out = Outcome::default();
    for line in lines(pool, ws, batch).await? {
        // Submitted with a receipt: the provider has it; reconcile settles it.
        let sendable = match line.state {
            Some(LineState::Pending | LineState::Failed) => true,
            Some(LineState::Submitted) => line.receipt.is_none(),
            _ => false,
        };
        if !sendable {
            out.submitted += 1;
            continue;
        }
        // Commit `submitted` and read the live amount before any provider IO.
        let cents: Option<i64> = sqlx::query_scalar(
            "UPDATE payouts p SET rail = $3, rail_state = 'submitted', rail_error = NULL \
             FROM payout_batches b JOIN programs pr ON pr.id = b.program_id \
             WHERE p.id = $1 AND p.batch_id = b.id AND pr.workspace_id = $2 \
               AND p.status = 'approved' AND p.amount_cents > 0 \
               AND p.rail_state IN ('pending', 'failed', 'submitted') \
             RETURNING p.amount_cents",
        )
        .bind(line.id)
        .bind(ws)
        .bind(rail.as_str())
        .fetch_optional(pool)
        .await?;
        let Some(cents) = cents else { continue };
        let dest = PayoutDestination {
            email: line.email.clone(),
        };
        match client
            .payout(line.id, &dest, Money::new(cents, currency))
            .await
        {
            Ok((receipt, RailStatus::Paid)) => {
                if settle_confirmed(pool, ws, batch, line.id, Some(&receipt.reference)).await? {
                    out.paid += 1;
                }
            }
            Ok((receipt, RailStatus::Pending)) => {
                record(
                    pool,
                    ws,
                    line.id,
                    Some(&receipt.reference),
                    None,
                    Some("pending"),
                    None,
                )
                .await?;
                out.submitted += 1;
            }
            Ok((receipt, RailStatus::Failed)) => {
                fail(
                    pool,
                    ws,
                    line.id,
                    Some(&receipt.reference),
                    "provider reported failure",
                )
                .await?;
                out.failed += 1;
            }
            Err(PayoutError::Rejected(m)) => {
                fail(pool, ws, line.id, None, &m).await?;
                out.failed += 1;
            }
            // Unknown outcome: stay submitted; the next dispatch re-sends the same key.
            Err(PayoutError::Transient(m)) => {
                record(pool, ws, line.id, None, None, None, Some(&m)).await?;
                out.submitted += 1;
            }
        }
    }
    out.batch_paid = close_batch_if_done(pool, ws, batch).await?;
    Ok(out)
}

async fn fail(
    pool: &PgPool,
    ws: Uuid,
    line: Uuid,
    receipt: Option<&str>,
    why: &str,
) -> Result<(), AppError> {
    record(
        pool,
        ws,
        line,
        receipt,
        Some(LineState::Failed),
        Some("failed"),
        Some(why),
    )
    .await
}

/// Rails with lines awaiting a provider answer in this batch.
pub async fn rails_in_flight(pool: &PgPool, ws: Uuid, batch: Uuid) -> Result<Vec<Rail>, AppError> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT p.rail FROM payouts p JOIN payout_batches b ON b.id = p.batch_id \
           JOIN programs pr ON pr.id = b.program_id \
         WHERE p.batch_id = $1 AND pr.workspace_id = $2 AND p.status = 'approved' \
           AND p.rail_state = 'submitted' AND p.rail_receipt IS NOT NULL",
    )
    .bind(batch)
    .bind(ws)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().filter_map(|r| Rail::parse(r)).collect())
}

/// Ask each provider about its submitted lines. Confirmed paid posts the ledger; failed lines
/// become retryable; pending stay. Closes the batch when every line is paid.
pub async fn reconcile(
    pool: &PgPool,
    ws: Uuid,
    batch: Uuid,
    clients: &HashMap<Rail, RailClient>,
) -> Result<Outcome, AppError> {
    let mut out = Outcome::default();
    let rows: Vec<(Uuid, String, Value)> = sqlx::query_as(
        "SELECT p.id, p.rail, p.rail_receipt FROM payouts p \
           JOIN payout_batches b ON b.id = p.batch_id JOIN programs pr ON pr.id = b.program_id \
         WHERE p.batch_id = $1 AND pr.workspace_id = $2 AND p.status = 'approved' \
           AND p.rail_state = 'submitted' AND p.rail_receipt IS NOT NULL ORDER BY p.id",
    )
    .bind(batch)
    .bind(ws)
    .fetch_all(pool)
    .await?;
    for (id, rail, receipt) in rows {
        let (Some(client), Some(reference)) = (
            Rail::parse(&rail).and_then(|r| clients.get(&r)),
            reference(&receipt),
        ) else {
            out.submitted += 1;
            continue;
        };
        match client
            .status(&PayoutReceipt {
                reference: reference.clone(),
            })
            .await
        {
            Ok(RailStatus::Paid) => {
                if settle_confirmed(pool, ws, batch, id, Some(&reference)).await? {
                    out.paid += 1;
                }
            }
            Ok(RailStatus::Failed) => {
                fail(pool, ws, id, None, "provider reported failure").await?;
                out.failed += 1;
            }
            Ok(RailStatus::Pending) => {
                record(pool, ws, id, None, None, Some("pending"), None).await?;
                out.submitted += 1;
            }
            Err(e) => {
                record(pool, ws, id, None, None, None, Some(&e.to_string())).await?;
                out.submitted += 1;
            }
        }
    }
    out.batch_paid = close_batch_if_done(pool, ws, batch).await?;
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/payouts/batches/{id}/reconcile",
            post_route(reconcile_route),
        )
        .route("/api/v1/payouts/rails", get(list_rails))
        .route(
            "/api/v1/payouts/rails/credentials",
            put(put_credentials).delete(delete_credentials),
        )
}

/// Body of `mark-paid` when a provider rail is chosen.
#[derive(Deserialize, ToSchema)]
pub struct MarkPaidRail {
    /// `manual_csv` (default), `paypal_payouts`, `wise` or `mock`.
    pub rail: String,
}

/// A batch dispatched or reconciled, with what this pass did.
#[derive(Serialize, ToSchema)]
pub struct RailResult {
    #[serde(flatten)]
    pub batch: BatchDetail,
    pub outcome: Outcome,
}

/// Redacted view of a rail: whether it can be selected, never its credentials.
#[derive(Serialize, ToSchema)]
pub struct RailView {
    pub rail: String,
    /// Can be chosen for `mark-paid`.
    pub available: bool,
    /// Credentials are stored (they are never returned).
    pub configured: bool,
    pub sandbox: Option<bool>,
}

#[derive(Deserialize, ToSchema)]
pub struct PutCredentials {
    /// `paypal_payouts` or `wise`.
    pub rail: String,
    /// `paypal_payouts`: `client_id`, `client_secret`. `wise`: `api_token`, `profile_id`.
    pub credentials: HashMap<String, String>,
    /// Use the provider sandbox (default true).
    pub sandbox: Option<bool>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RailQuery {
    /// `paypal_payouts` or `wise`.
    pub rail: String,
}

fn rail_param(s: &str) -> Result<Rail, AppError> {
    Rail::parse(s)
        .filter(|r| r.needs_credentials())
        .ok_or(AppError::NotFound("payout rail not found"))
}

/// `mark-paid` through a provider: see [`dispatch`].
pub(crate) async fn mark_paid_via(
    s: &AppState,
    ws: Uuid,
    batch: Uuid,
    rail: Rail,
) -> Result<Outcome, AppError> {
    // Fail before any state change when the rail cannot be used.
    let client = load_client(&s.pool, &s.vault, ws, rail).await?;
    dispatch(&s.pool, ws, batch, rail, &client).await
}

/// Ask providers about submitted payout lines and settle the confirmed ones.
#[utoipa::path(
    post,
    path = "/api/v1/payouts/batches/{id}/reconcile",
    tag = "payouts",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "BatchView id")),
    responses(
        (status = 200, body = RailResult),
        (status = 404, body = ErrorBody),
        (status = 422, description = "A rail lost its credentials", body = ErrorBody),
    )
)]
pub async fn reconcile_route(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    Path(id): Path<Uuid>,
) -> Result<Json<RailResult>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    payouts::require_batch(&s.pool, ws, id).await?;
    let mut clients = HashMap::new();
    for rail in rails_in_flight(&s.pool, ws, id).await? {
        clients.insert(rail, load_client(&s.pool, &s.vault, ws, rail).await?);
    }
    let outcome = reconcile(&s.pool, ws, id, &clients).await?;
    audit::record(
        &s.pool,
        &auth.principal,
        "payout_batch.reconcile",
        "payout_batch",
        id,
        Some(json!({"paid": outcome.paid, "failed": outcome.failed})),
    )
    .await?;
    Ok(Json(RailResult {
        batch: payouts::detail(&s.pool, ws, id).await?,
        outcome,
    }))
}

/// Payout rails and whether each is usable. Credentials are never returned.
#[utoipa::path(
    get,
    path = "/api/v1/payouts/rails",
    tag = "payouts",
    security(("cookie" = [])),
    responses((status = 200, body = Vec<RailView>))
)]
pub async fn list_rails(
    State(s): State<AppState>,
    auth: Auth<Admin>,
) -> Result<Json<Vec<RailView>>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rows: Vec<(String, bool)> =
        sqlx::query_as("SELECT rail, sandbox FROM payout_rail_credentials WHERE workspace_id = $1")
            .bind(ws)
            .fetch_all(&s.pool)
            .await?;
    let mut out = vec![
        RailView {
            rail: "manual_csv".into(),
            available: true,
            configured: true,
            sandbox: None,
        },
        RailView {
            rail: "mock".into(),
            available: true,
            configured: true,
            sandbox: None,
        },
    ];
    for r in [Rail::PaypalPayouts, Rail::Wise] {
        let found = rows.iter().find(|(n, _)| n == r.as_str());
        out.push(RailView {
            rail: r.as_str().into(),
            available: found.is_some(),
            configured: found.is_some(),
            sandbox: found.map(|f| f.1),
        });
    }
    Ok(Json(out))
}

/// Store (sealed) the workspace's credentials for a provider rail, replacing any existing.
#[utoipa::path(
    put,
    path = "/api/v1/payouts/rails/credentials",
    tag = "payouts",
    security(("cookie" = [])),
    request_body = PutCredentials,
    responses(
        (status = 200, body = RailView),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
pub async fn put_credentials(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    AppJson(req): AppJson<PutCredentials>,
) -> Result<Json<RailView>, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rail = rail_param(&req.rail)?;
    let want: HashSet<&str> = cred_keys(rail).iter().copied().collect();
    let got: HashSet<&str> = req.credentials.keys().map(String::as_str).collect();
    if want != got
        || req
            .credentials
            .values()
            .any(|v| v.trim().is_empty() || v.len() > 512)
    {
        return Err(AppError::BadRequest(format!(
            "credentials must be exactly: {}",
            cred_keys(rail).join(", ")
        )));
    }
    let sandbox = req.sandbox.unwrap_or(true);
    let plain = serde_json::to_string(&req.credentials).map_err(|_| AppError::Internal)?;
    let mut tx = s.pool.begin().await?;
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM payout_rail_credentials WHERE workspace_id = $1 AND rail = $2 FOR UPDATE",
    )
    .bind(ws)
    .bind(rail.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let id = existing.unwrap_or_else(Uuid::now_v7);
    let sealed = s.vault.seal_col(CRED_TABLE, "creds", id, &plain);
    let sealed_now = bf_core::secrets::KeyRing::is_sealed(&sealed);
    sqlx::query(
        "INSERT INTO payout_rail_credentials (id, workspace_id, rail, sandbox, creds, creds_enc_at) \
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 THEN now() END) \
         ON CONFLICT (id) DO UPDATE SET sandbox = $4, creds = $5, updated_at = now(), \
           creds_enc_at = CASE WHEN $6 THEN now() END",
    )
    .bind(id)
    .bind(ws)
    .bind(rail.as_str())
    .bind(sandbox)
    .bind(&sealed)
    .bind(sealed_now)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut *tx,
        &auth.principal,
        "payout_rail.set_credentials",
        "payout_rail",
        id,
        Some(json!({"rail": rail.as_str(), "sandbox": sandbox})),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(RailView {
        rail: rail.as_str().into(),
        available: true,
        configured: true,
        sandbox: Some(sandbox),
    }))
}

/// Remove the workspace's credentials for a provider rail.
#[utoipa::path(
    delete,
    path = "/api/v1/payouts/rails/credentials",
    tag = "payouts",
    security(("cookie" = [])),
    params(RailQuery),
    responses((status = 204), (status = 404, body = ErrorBody))
)]
pub async fn delete_credentials(
    State(s): State<AppState>,
    auth: Auth<Admin>,
    axum::extract::Query(q): axum::extract::Query<RailQuery>,
) -> Result<axum::http::StatusCode, AppError> {
    let ws = auth.principal.workspace_scope(None)?;
    let rail = rail_param(&q.rail)?;
    let id: Option<Uuid> = sqlx::query_scalar(
        "DELETE FROM payout_rail_credentials WHERE workspace_id = $1 AND rail = $2 RETURNING id",
    )
    .bind(ws)
    .bind(rail.as_str())
    .fetch_optional(&s.pool)
    .await?;
    let id = id.ok_or(AppError::NotFound("payout rail not configured"))?;
    audit::record(
        &s.pool,
        &auth.principal,
        "payout_rail.delete_credentials",
        "payout_rail",
        id,
        Some(json!({"rail": rail.as_str()})),
    )
    .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
