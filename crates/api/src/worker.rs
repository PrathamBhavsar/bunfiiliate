//! Background worker: turns `inbound_events` into conversions and approves held ones.
//!
//! Events are claimed one per transaction with `FOR UPDATE SKIP LOCKED`; the conversion,
//! its ledger txn and the outbox row commit together with the event's `processed_at`.
//! Ticks (approve, rollup) are guarded by a transaction-scoped advisory lock.

use std::time::Duration;

use bf_core::{
    Account, AffiliateId, Attribution, Bps, CommissionKind, CommissionPlan, ConversionEvent,
    ConversionStatus, Currency, Duration as Window, FirstTouch, LastTouch, LedgerTxn, Money, Touch,
    conversion_created,
};
use bf_db::ledger::{self, NewEntry};
use chrono::{DateTime, Datelike, Months, Utc};
use serde_json::{Value, json};
use sqlx::{Acquire, PgConnection, PgPool};
use uuid::Uuid;

use crate::analytics;
use crate::error::AppError;

const INGEST_EVERY: Duration = Duration::from_secs(1);
const APPROVE_EVERY: Duration = Duration::from_secs(60);
const ROLLUP_EVERY: Duration = Duration::from_secs(60);
/// Predicate on an `affiliates a` row: active and enrolled in program `$2`.
const ELIGIBLE: &str = "a.status = 'active' \
     AND EXISTS (SELECT 1 FROM enrollments e WHERE e.affiliate_id = a.id AND e.program_id = $2)";
const APPROVE_BATCH: i64 = 100;
/// Arbitrary key shared by every worker so only one runs the approve tick at a time.
const APPROVE_LOCK: i64 = 0x6266_6170;

/// Run both loops until the task is dropped.
pub async fn run(pool: PgPool) {
    let mut ingest = tokio::time::interval(INGEST_EVERY);
    let mut approve = tokio::time::interval(APPROVE_EVERY);
    let mut rollup = tokio::time::interval(ROLLUP_EVERY);
    loop {
        tokio::select! {
            _ = ingest.tick() => match process_events(&pool).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(events = n, "ingested"),
                Err(e) => tracing::error!(error = %e, "ingest tick failed"),
            },
            _ = approve.tick() => match approve_due(&pool).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(conversions = n, "auto-approved"),
                Err(e) => tracing::error!(error = %e, "approve tick failed"),
            },
            _ = rollup.tick() => if let Err(e) = analytics::rollup(&pool).await {
                tracing::error!(error = %e, "rollup tick failed");
            },
        }
    }
}

/// Process every unprocessed event; returns how many were claimed.
pub async fn process_events(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut n = 0;
    while process_next(pool).await? {
        n += 1;
    }
    Ok(n)
}

#[derive(sqlx::FromRow)]
struct Event {
    id: Uuid,
    program_id: Uuid,
    source: String,
    external_id: String,
    payload: String,
    received_at: DateTime<Utc>,
}

/// A reason the event cannot become a conversion; recorded on the event, not retried.
pub(crate) enum Fail {
    Rejected(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        Fail::Db(e)
    }
}

impl From<Fail> for AppError {
    fn from(f: Fail) -> Self {
        match f {
            Fail::Rejected(_) => AppError::Conflict("conversion cannot take this action"),
            Fail::Db(e) => e.into(),
        }
    }
}

fn reject<T>(why: impl Into<String>) -> Result<T, Fail> {
    Err(Fail::Rejected(why.into()))
}

/// Integrity (23xxx) and data (22xxx) errors come from the event's content: retrying
/// cannot help and would wedge the queue. Everything else is transient.
fn is_data_error(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c.starts_with("22") || c.starts_with("23"))
}

async fn process_next(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let Some(ev) = sqlx::query_as::<_, Event>(
        "SELECT id, program_id, source::text AS source, external_id, payload::text AS payload, \
                received_at \
         FROM inbound_events WHERE processed_at IS NULL \
         ORDER BY received_at, id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };

    // Savepoint: a rejected event rolls back its partial writes but still gets marked.
    let mut sp = tx.begin().await?;
    let error = match handle(&mut sp, &ev).await {
        Ok(()) => {
            sp.commit().await?;
            None
        }
        Err(Fail::Rejected(why)) => {
            sp.rollback().await?;
            Some(why)
        }
        Err(Fail::Db(e)) if is_data_error(&e) => {
            sp.rollback().await?;
            Some(e.to_string())
        }
        Err(Fail::Db(e)) => return Err(e),
    };
    if let Some(why) = &error {
        tracing::warn!(event = %ev.id, reason = %why, "event failed");
    }
    sqlx::query("UPDATE inbound_events SET processed_at = now(), error = $2 WHERE id = $1")
        .bind(ev.id)
        .bind(error)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// The conversion facts an event carries, whatever its source.
struct Fields {
    customer: String,
    amount_cents: i64,
    currency: Option<String>,
    conversion_type: Option<String>,
    click: Option<Uuid>,
    code: Option<String>,
    occurred_at: DateTime<Utc>,
}

fn text(p: &Value, ptr: &str) -> Option<String> {
    p.pointer(ptr)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Integer cents as a JSON number, or as a numeric string (Paddle).
fn cents(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn rfc3339(p: &Value, ptr: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(p.pointer(ptr)?.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// `Ok(None)`: a valid event that is not a conversion (other Stripe/Paddle types).
fn parse(source: &str, p: &Value, received_at: DateTime<Utc>) -> Result<Option<Fields>, String> {
    let kind = |ptr: &str| p.pointer(ptr).and_then(Value::as_str);
    let (customer, amount, currency, click, code, ty, at) = match source {
        "stripe" => {
            let amount = match kind("/type") {
                Some("invoice.paid") => "/data/object/amount_paid",
                Some("checkout.session.completed") => "/data/object/amount_total",
                _ => return Ok(None),
            };
            (
                text(p, "/data/object/customer"),
                cents(p.pointer(amount)),
                text(p, "/data/object/currency"),
                text(p, "/data/object/metadata/bf_click"),
                text(p, "/data/object/metadata/bf_ref"),
                None,
                p.get("created")
                    .and_then(Value::as_i64)
                    .and_then(|s| DateTime::from_timestamp(s, 0)),
            )
        }
        "paddle" => {
            if kind("/event_type") != Some("transaction.completed") {
                return Ok(None);
            }
            (
                text(p, "/data/customer_id"),
                cents(p.pointer("/data/details/totals/total")),
                text(p, "/data/currency_code"),
                text(p, "/data/custom_data/bf_click"),
                text(p, "/data/custom_data/bf_ref"),
                None,
                rfc3339(p, "/occurred_at"),
            )
        }
        _ => (
            text(p, "/customer"),
            cents(p.get("amount_cents")),
            text(p, "/currency"),
            text(p, "/click"),
            text(p, "/code"),
            text(p, "/type"),
            rfc3339(p, "/occurred_at"),
        ),
    };
    Ok(Some(Fields {
        customer: customer.ok_or("missing customer")?,
        amount_cents: amount.ok_or("missing amount")?,
        currency: currency.map(|c| c.to_uppercase()),
        conversion_type: ty,
        click: click.and_then(|c| Uuid::parse_str(&c).ok()),
        code,
        occurred_at: at.unwrap_or(received_at),
    }))
}

/// Whole months from `from` to `to` (0 = the first month).
fn months_between(from: DateTime<Utc>, to: DateTime<Utc>) -> u32 {
    if to <= from {
        return 0;
    }
    let mut m = (to.year() - from.year()) * 12 + to.month() as i32 - from.month() as i32;
    while m > 0
        && from
            .checked_add_months(Months::new(m as u32))
            .is_none_or(|t| t > to)
    {
        m -= 1;
    }
    m.max(0) as u32
}

#[derive(sqlx::FromRow)]
struct ProgramRow {
    currency: String,
    attribution_model: String,
    cookie_days: i32,
}

#[derive(sqlx::FromRow)]
struct PlanRow {
    kind: String,
    rate_bps: Option<i32>,
    amount_cents: Option<i64>,
    duration: String,
    months: Option<i32>,
}

impl PlanRow {
    fn plan(&self) -> Option<CommissionPlan> {
        let kind = match self.kind.as_str() {
            "percent" => CommissionKind::Percent(Bps::new(u32::try_from(self.rate_bps?).ok()?)?),
            _ => CommissionKind::Flat(self.amount_cents?),
        };
        let duration = match self.duration.as_str() {
            "one_month" => Window::OneMonth,
            "x_months" => Window::XMonths(u32::try_from(self.months?).ok()?),
            _ => Window::Lifetime,
        };
        Some(CommissionPlan { kind, duration })
    }
}

async fn handle(c: &mut PgConnection, ev: &Event) -> Result<(), Fail> {
    let payload: Value =
        serde_json::from_str(&ev.payload).map_err(|e| Fail::Rejected(e.to_string()))?;
    if let Some(r) = reversal(&ev.source, &payload) {
        return reverse(c, ev, r).await;
    }
    let Some(f) = parse(&ev.source, &payload, ev.received_at).map_err(Fail::Rejected)? else {
        return Ok(());
    };
    let program: ProgramRow = sqlx::query_as(
        "SELECT currency, attribution_model::text AS attribution_model, cookie_days \
         FROM programs WHERE id = $1",
    )
    .bind(ev.program_id)
    .fetch_one(&mut *c)
    .await?;
    if f.currency.as_deref().is_some_and(|x| x != program.currency) {
        return reject("currency does not match program");
    }
    let Some(currency) = Currency::new(&program.currency) else {
        return reject("invalid program currency");
    };

    let known: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT affiliate_id, attributed_at FROM customers \
         WHERE program_id = $1 AND external_id = $2",
    )
    .bind(ev.program_id)
    .bind(&f.customer)
    .fetch_optional(&mut *c)
    .await?;
    // Recurring conversions stay with the customer's affiliate.
    let (affiliate, first_click, is_new) = match known {
        Some((Some(a), _)) => (a, None, false),
        _ => {
            let touches = touches(c, ev.program_id, program.cookie_days, &f).await?;
            let model: &dyn Attribution = match program.attribution_model.as_str() {
                "first_touch" => &FirstTouch,
                _ => &LastTouch,
            };
            match model.pick(&touches) {
                Some(t) => (t.affiliate.0, t.click_id, true),
                None => return Ok(()), // organic: nothing to credit
            }
        }
    };

    // Candidates are already eligible; this guards the recurring path (suspended or removed).
    let eligible: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM affiliates a WHERE a.id = $1 AND {ELIGIBLE})"
    ))
    .bind(affiliate)
    .bind(ev.program_id)
    .fetch_one(&mut *c)
    .await?;
    if !eligible {
        return Ok(());
    }

    // The upsert returns the winning attribution if another worker got there first.
    let (customer_id, affiliate, attributed_at) = if is_new {
        sqlx::query_as::<_, (Uuid, Uuid, DateTime<Utc>)>(
            "INSERT INTO customers (id, program_id, external_id, affiliate_id, first_click_id, attributed_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (program_id, external_id) DO UPDATE SET \
               affiliate_id = COALESCE(customers.affiliate_id, EXCLUDED.affiliate_id), \
               first_click_id = CASE WHEN customers.affiliate_id IS NULL \
                                THEN EXCLUDED.first_click_id ELSE customers.first_click_id END, \
               attributed_at = CASE WHEN customers.affiliate_id IS NULL \
                                THEN EXCLUDED.attributed_at ELSE customers.attributed_at END \
             RETURNING id, affiliate_id, attributed_at",
        )
        .bind(Uuid::now_v7())
        .bind(ev.program_id)
        .bind(&f.customer)
        .bind(affiliate)
        .bind(first_click)
        .bind(f.occurred_at)
        .fetch_one(&mut *c)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, affiliate_id, COALESCE(attributed_at, $3) FROM customers \
             WHERE program_id = $1 AND external_id = $2",
        )
        .bind(ev.program_id)
        .bind(&f.customer)
        .bind(f.occurred_at)
        .fetch_one(&mut *c)
        .await?
    };

    let type_id: Option<Uuid> = match &f.conversion_type {
        Some(key) => match sqlx::query_scalar(
            "SELECT id FROM conversion_types WHERE program_id = $1 AND key = $2",
        )
        .bind(ev.program_id)
        .bind(key)
        .fetch_optional(&mut *c)
        .await?
        {
            Some(id) => Some(id),
            None => return reject(format!("unknown conversion type {key}")),
        },
        None => None,
    };

    // Plan: the affiliate's enrollment override, else the conversion type's plan, else the
    // program default.
    let plan: Option<PlanRow> = sqlx::query_as(
        "SELECT kind::text AS kind, rate_bps, amount_cents, duration::text AS duration, months \
         FROM commission_plans WHERE id = COALESCE( \
           (SELECT plan_override_id FROM enrollments WHERE affiliate_id = $1 AND program_id = $2), \
           (SELECT id FROM commission_plans WHERE conversion_type_id = $3 ORDER BY id LIMIT 1), \
           (SELECT default_plan_id FROM programs WHERE id = $2))",
    )
    .bind(affiliate)
    .bind(ev.program_id)
    .bind(type_id)
    .fetch_optional(&mut *c)
    .await?;
    let Some(plan) = plan.as_ref().and_then(PlanRow::plan) else {
        return reject("no commission plan");
    };
    let amount = Money::new(f.amount_cents, currency);
    let commission = plan
        .commission_for(amount, months_between(attributed_at, f.occurred_at))
        .map_err(|e| Fail::Rejected(e.to_string()))?;

    let conversion = Uuid::now_v7();
    let created = sqlx::query(
        "INSERT INTO conversions (id, program_id, customer_id, affiliate_id, conversion_type_id, \
           external_id, amount_cents, commission_cents, currency, occurred_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         ON CONFLICT (program_id, external_id) DO NOTHING",
    )
    .bind(conversion)
    .bind(ev.program_id)
    .bind(customer_id)
    .bind(affiliate)
    .bind(type_id)
    .bind(&ev.external_id)
    .bind(amount.cents)
    .bind(commission.cents)
    .bind(&program.currency)
    .bind(f.occurred_at)
    .execute(&mut *c)
    .await?
    .rows_affected();
    if created == 0 {
        return Ok(()); // already converted: replay or redelivery under another source
    }

    let txn = conversion_created(commission).map_err(|e| Fail::Rejected(e.to_string()))?;
    post(c, &txn, ev.program_id, affiliate, (Some(conversion), None)).await?;
    outbox(
        c,
        "conversion.created",
        json!({
            "conversion_id": conversion,
            "program_id": ev.program_id,
            "affiliate_id": affiliate,
            "amount_cents": amount.cents,
            "commission_cents": commission.cents,
            "currency": program.currency,
            "status": "pending",
        }),
    )
    .await?;
    Ok(())
}

/// A refund or chargeback that points at an earlier conversion.
struct Reversal {
    reason: &'static str,
    target: Target,
}

enum Target {
    /// Generic events name the original event's external id.
    External(String),
    /// Stripe/Paddle events carry provider ids found in the original conversion event.
    Refs(Vec<String>),
}

/// `None`: not a refund or chargeback.
fn reversal(source: &str, p: &Value) -> Option<Reversal> {
    let kind = |ptr: &str| p.pointer(ptr).and_then(Value::as_str);
    let refs = |ptrs: &[&str]| Target::Refs(ptrs.iter().filter_map(|x| text(p, x)).collect());
    let (reason, target) = match (source, kind("/type"), kind("/event_type")) {
        ("stripe", Some("charge.refunded"), _) => (
            "refund",
            refs(&[
                "/data/object/id",
                "/data/object/invoice",
                "/data/object/payment_intent",
            ]),
        ),
        ("stripe", Some("charge.dispute.created"), _) => (
            "chargeback",
            refs(&["/data/object/charge", "/data/object/payment_intent"]),
        ),
        ("paddle", _, Some("adjustment.created")) => (
            match kind("/data/action")? {
                "refund" => "refund",
                "chargeback" => "chargeback",
                _ => return None,
            },
            refs(&["/data/transaction_id"]),
        ),
        ("generic", Some(t @ ("refund" | "chargeback")), _) => (
            if t == "refund" {
                "refund"
            } else {
                "chargeback"
            },
            Target::External(text(p, "/conversion")?),
        ),
        _ => return None,
    };
    Some(Reversal { reason, target })
}

/// Columns of a conversion `c` about to change status (select it `FOR UPDATE OF c`).
pub(crate) const LOCKED_COLS: &str = "c.id, c.program_id, c.affiliate_id, c.commission_cents, \
     c.currency, c.status::text AS status, c.payout_id";

#[derive(sqlx::FromRow)]
pub(crate) struct Locked {
    id: Uuid,
    program_id: Uuid,
    affiliate_id: Uuid,
    commission_cents: i64,
    currency: String,
    pub(crate) status: String,
    payout_id: Option<Uuid>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    Approve,
    Reject,
    Reverse,
}

/// Move a locked conversion through the state machine: status column, balanced ledger txn,
/// shrink of a payout not yet paid (it was sized with this commission; a paid one nets
/// against the next batch) and the outbox row. Returns the new status.
pub(crate) async fn settle(
    c: &mut PgConnection,
    conv: &Locked,
    act: Act,
    reason: &str,
) -> Result<&'static str, Fail> {
    let from = match conv.status.as_str() {
        "pending" => ConversionStatus::Pending,
        "approved" => ConversionStatus::Approved,
        "paid" => ConversionStatus::Paid,
        "rejected" => ConversionStatus::Rejected,
        _ => ConversionStatus::Reversed,
    };
    let Some(currency) = Currency::new(&conv.currency) else {
        return reject("invalid conversion currency");
    };
    let m = Money::new(conv.commission_cents, currency);
    let event = match act {
        Act::Approve => ConversionEvent::Approve(m),
        Act::Reject => ConversionEvent::Reject(m),
        Act::Reverse => ConversionEvent::Reverse(m),
    };
    let (_, txn) = from
        .transition(event)
        .map_err(|e| Fail::Rejected(e.to_string()))?;
    let (to, stamp) = match act {
        Act::Approve => ("approved", "approved_at"),
        Act::Reject => ("rejected", "reversed_at"),
        Act::Reverse => ("reversed", "reversed_at"),
    };
    sqlx::query(&format!(
        "UPDATE conversions SET status = $2::conversion_status, {stamp} = now() WHERE id = $1"
    ))
    .bind(conv.id)
    .bind(to)
    .execute(&mut *c)
    .await?;
    if let Some(txn) = txn {
        post(
            c,
            &txn,
            conv.program_id,
            conv.affiliate_id,
            (Some(conv.id), None),
        )
        .await?;
    }
    if let Some(payout) = conv.payout_id {
        sqlx::query(
            "UPDATE payouts SET amount_cents = amount_cents - $2 \
             WHERE id = $1 AND status IN ('draft', 'approved')",
        )
        .bind(payout)
        .bind(conv.commission_cents)
        .execute(&mut *c)
        .await?;
    }
    let mut payload = json!({
        "conversion_id": conv.id,
        "program_id": conv.program_id,
        "affiliate_id": conv.affiliate_id,
        "commission_cents": conv.commission_cents,
        "currency": conv.currency,
        "status": to,
    });
    let topic = if act == Act::Approve {
        "conversion.approved"
    } else {
        payload["reason"] = json!(reason);
        "conversion.reversed"
    };
    outbox(c, topic, payload).await?;
    Ok(to)
}

/// Undo a conversion's commission: pending is rejected, approved or paid is reversed. A paid
/// conversion leaves the affiliate's payable negative, netted against their next payout.
/// Unknown (organic) and already reversed conversions are a no-op, so replays are safe.
async fn reverse(c: &mut PgConnection, ev: &Event, r: Reversal) -> Result<(), Fail> {
    let conv: Option<Locked> = match &r.target {
        Target::External(id) => {
            sqlx::query_as(&format!(
                "SELECT {LOCKED_COLS} FROM conversions c \
                 WHERE c.program_id = $1 AND c.external_id = $2 FOR UPDATE OF c"
            ))
            .bind(ev.program_id)
            .bind(id)
            .fetch_optional(&mut *c)
            .await?
        }
        Target::Refs(refs) => {
            sqlx::query_as(&format!(
                "SELECT {LOCKED_COLS} FROM inbound_events e \
                   JOIN conversions c ON c.program_id = e.program_id AND c.external_id = e.external_id \
                 WHERE e.program_id = $1 AND e.source = $2::event_source AND ( \
                   e.payload #>> '{{data,object,id}}' = ANY($3) \
                   OR e.payload #>> '{{data,object,charge}}' = ANY($3) \
                   OR e.payload #>> '{{data,object,payment_intent}}' = ANY($3) \
                   OR e.payload #>> '{{data,id}}' = ANY($3)) \
                 ORDER BY e.received_at, e.id LIMIT 1 FOR UPDATE OF c"
            ))
            .bind(ev.program_id)
            .bind(&ev.source)
            .bind(refs)
            .fetch_optional(&mut *c)
            .await?
        }
    };
    let Some(conv) = conv else {
        return Ok(());
    };
    let act = match conv.status.as_str() {
        "pending" => Act::Reject,
        "approved" | "paid" => Act::Reverse,
        _ => return Ok(()), // already rejected or reversed
    };
    settle(c, &conv, act, r.reason).await?;
    Ok(())
}

/// Candidate touches: the clicks of the visitor behind the event's click inside the cookie
/// window, else the bare referral code.
async fn touches(
    c: &mut PgConnection,
    program: Uuid,
    cookie_days: i32,
    f: &Fields,
) -> Result<Vec<Touch>, sqlx::Error> {
    let rows: Vec<(Uuid, Uuid, i64)> = sqlx::query_as(
        "SELECT k.id, k.affiliate_id, (extract(epoch FROM k.ts) * 1000)::bigint \
         FROM clicks k JOIN affiliates a ON a.id = k.affiliate_id \
         WHERE k.program_id = $1 \
           AND k.visitor_id = (SELECT visitor_id FROM clicks WHERE id = $2 AND program_id = $1) \
           AND k.ts <= $3 AND k.ts > $3 - make_interval(days => $4) \
           AND a.status = 'active' \
           AND EXISTS (SELECT 1 FROM enrollments e WHERE e.affiliate_id = a.id AND e.program_id = $1) \
         ORDER BY k.ts, k.id",
    )
    .bind(program)
    .bind(f.click)
    .bind(f.occurred_at)
    .bind(cookie_days)
    .fetch_all(&mut *c)
    .await?;
    if !rows.is_empty() {
        return Ok(rows
            .into_iter()
            .map(|(id, a, at_ms)| Touch {
                click_id: Some(id),
                affiliate: AffiliateId(a),
                at_ms,
            })
            .collect());
    }
    let by_code: Option<Uuid> = match &f.code {
        Some(code) => {
            sqlx::query_scalar(&format!(
                "SELECT r.affiliate_id FROM referral_codes r JOIN affiliates a ON a.id = r.affiliate_id \
                 WHERE r.code = $1 AND r.program_id = $2 AND {ELIGIBLE}"
            ))
            .bind(code)
            .bind(program)
            .fetch_optional(&mut *c)
            .await?
        }
        None => None,
    };
    Ok(by_code
        .map(|a| Touch {
            click_id: None,
            affiliate: AffiliateId(a),
            at_ms: f.occurred_at.timestamp_millis(),
        })
        .into_iter()
        .collect())
}

fn db_account(a: Account) -> ledger::Account {
    match a {
        Account::Expense => ledger::Account::Expense,
        Account::AffPending => ledger::Account::AffPending,
        Account::AffPayable => ledger::Account::AffPayable,
        Account::Cash => ledger::Account::Cash,
    }
}

/// Write a state-machine txn to the ledger and return its id. Zero-amount txns carry no
/// money, so skip them.
pub(crate) async fn post(
    c: &mut PgConnection,
    txn: &LedgerTxn,
    program: Uuid,
    affiliate: Uuid,
    (conversion, payout): (Option<Uuid>, Option<Uuid>),
) -> Result<Option<Uuid>, sqlx::Error> {
    if txn.postings().iter().all(|p| p.amount.cents == 0) {
        return Ok(None);
    }
    let entries: Vec<NewEntry> = txn
        .postings()
        .iter()
        .map(|p| NewEntry {
            account: db_account(p.account),
            program_id: program,
            affiliate_id: (p.account != Account::Expense).then_some(affiliate),
            amount_cents: p.amount.cents,
            currency: p.amount.currency.as_str().to_owned(),
            conversion_id: conversion,
            payout_id: payout,
        })
        .collect();
    let id = Uuid::now_v7();
    ledger::insert_entries(c, id, &entries).await?;
    Ok(Some(id))
}

pub(crate) async fn outbox(
    c: &mut PgConnection,
    topic: &str,
    payload: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO outbox (topic, payload) VALUES ($1, $2::text::jsonb)")
        .bind(topic)
        .bind(payload.to_string())
        .execute(c)
        .await?;
    Ok(())
}

/// Approve pending conversions past their program's hold period; returns how many.
pub async fn approve_due(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut total = 0;
    loop {
        let n = approve_batch(pool).await?;
        total += n;
        if n < APPROVE_BATCH as u64 {
            return Ok(total);
        }
    }
}

async fn approve_batch(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(APPROVE_LOCK)
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Ok(0);
    }
    let due: Vec<Locked> = sqlx::query_as(&format!(
        "SELECT {LOCKED_COLS} \
         FROM conversions c JOIN programs p ON p.id = c.program_id \
           JOIN affiliates a ON a.id = c.affiliate_id \
         WHERE c.status = 'pending' AND p.auto_approve \
           AND a.status = 'active' \
           AND EXISTS (SELECT 1 FROM enrollments e WHERE e.affiliate_id = a.id AND e.program_id = c.program_id) \
           AND c.occurred_at <= now() - make_interval(days => p.hold_days) \
         ORDER BY c.occurred_at, c.id LIMIT $1 FOR UPDATE OF c SKIP LOCKED"
    ))
    .bind(APPROVE_BATCH)
    .fetch_all(&mut *tx)
    .await?;
    for d in &due {
        match settle(&mut tx, d, Act::Approve, "").await {
            Ok(_) | Err(Fail::Rejected(_)) => {}
            Err(Fail::Db(e)) => return Err(e),
        }
    }
    tx.commit().await?;
    Ok(due.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn whole_months() {
        let from = at("2026-01-31T10:00:00Z");
        for (to, want) in [
            ("2026-01-31T10:00:00Z", 0),
            ("2026-02-28T09:59:59Z", 0),
            ("2026-02-28T10:00:00Z", 1),
            ("2026-03-31T09:00:00Z", 1),
            ("2026-03-31T10:00:00Z", 2),
            ("2027-01-31T10:00:00Z", 12),
            ("2025-12-01T00:00:00Z", 0),
        ] {
            assert_eq!(months_between(from, at(to)), want, "{to}");
        }
    }

    #[test]
    fn parses_each_source() {
        let t = at("2026-05-01T00:00:00Z");
        let g = parse(
            "generic",
            &json!({"customer": "c", "amount_cents": 500, "currency": "usd", "type": "signup",
                    "occurred_at": "2026-04-01T00:00:00Z"}),
            t,
        )
        .unwrap()
        .unwrap();
        assert_eq!((g.amount_cents, g.currency.as_deref()), (500, Some("USD")));
        assert_eq!(g.occurred_at, at("2026-04-01T00:00:00Z"));
        assert_eq!(g.conversion_type.as_deref(), Some("signup"));

        let s = parse(
            "stripe",
            &json!({"type": "invoice.paid", "created": 1_700_000_000,
                    "data": {"object": {"customer": "cus_1", "amount_paid": 4900, "currency": "usd",
                             "metadata": {"bf_ref": "CODE"}}}}),
            t,
        )
        .unwrap()
        .unwrap();
        assert_eq!((s.customer.as_str(), s.amount_cents), ("cus_1", 4900));
        assert_eq!(s.code.as_deref(), Some("CODE"));

        let p = parse(
            "paddle",
            &json!({"event_type": "transaction.completed", "occurred_at": "2026-04-02T00:00:00Z",
                    "data": {"customer_id": "ctm_1", "currency_code": "USD",
                             "details": {"totals": {"total": "1250"}}}}),
            t,
        )
        .unwrap()
        .unwrap();
        assert_eq!((p.customer.as_str(), p.amount_cents), ("ctm_1", 1250));

        assert!(
            parse("stripe", &json!({"type": "customer.created"}), t)
                .unwrap()
                .is_none()
        );
        assert!(parse("generic", &json!({"amount_cents": 1}), t).is_err());
        assert!(parse("generic", &json!({"customer": "c"}), t).is_err());
    }
}
