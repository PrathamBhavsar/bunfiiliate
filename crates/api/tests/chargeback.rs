//! B02: refund / dispute / chargeback webhooks reverse the conversion they point at, matched
//! through provider ids (the reversal event's own id never equals the sale's event id).
//! Fixtures in `tests/fixtures` carry the exact Stripe/Paddle shapes the worker reads.

use api::worker::{approve_due, process_events};
use bf_db::tracker::{InboundEvent, insert_events};
use chrono::Utc;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

const STRIPE_SALE: &str = include_str!("fixtures/stripe_invoice_paid.json");
const STRIPE_REFUND: &str = include_str!("fixtures/stripe_charge_refunded.json");
const STRIPE_DISPUTE: &str = include_str!("fixtures/stripe_dispute_created.json");
const PADDLE_SALE: &str = include_str!("fixtures/paddle_transaction_completed.json");
const PADDLE_REFUND: &str = include_str!("fixtures/paddle_adjustment_refund.json");
const PADDLE_CHARGEBACK: &str = include_str!("fixtures/paddle_adjustment_chargeback.json");

struct Seed {
    program: Uuid,
    aff: Uuid,
}

async fn seed(pool: &PgPool, auto_approve: bool) -> Seed {
    let (ws, program, user, aff) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let q = |sql: &'static str| sqlx::query(sql);
    q("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', $2)")
        .bind(ws)
        .bind(ws.to_string())
        .execute(pool)
        .await
        .unwrap();
    q("INSERT INTO programs (id, workspace_id, name, currency, auto_approve, hold_days, public_key, webhook_secret) \
       VALUES ($1, $2, 'p', 'USD', $3, 0, $4, 's')")
        .bind(program).bind(ws).bind(auto_approve).bind(program.to_string()).execute(pool).await.unwrap();
    let plan = Uuid::now_v7();
    q("INSERT INTO commission_plans (id, program_id, kind, rate_bps, duration) VALUES ($1, $2, 'percent', 1000, 'lifetime')")
        .bind(plan).bind(program).execute(pool).await.unwrap();
    q("UPDATE programs SET default_plan_id = $2 WHERE id = $1")
        .bind(program)
        .bind(plan)
        .execute(pool)
        .await
        .unwrap();
    q("INSERT INTO users (id, workspace_id, email, password_hash, role) VALUES ($1, $2, $3, 'h', 'affiliate')")
        .bind(user).bind(ws).bind(format!("{user}@x.io")).execute(pool).await.unwrap();
    q("INSERT INTO affiliates (id, workspace_id, user_id, status) VALUES ($1, $2, $3, 'active')")
        .bind(aff)
        .bind(ws)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    q("INSERT INTO enrollments (affiliate_id, program_id) VALUES ($1, $2)")
        .bind(aff)
        .bind(program)
        .execute(pool)
        .await
        .unwrap();
    q("INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) VALUES ('CBCODE', $1, $2, 'https://x.io/')")
        .bind(aff).bind(program).execute(pool).await.unwrap();
    Seed { program, aff }
}

/// Insert a fixture as a received event; its external id is the event's own id, like the tracker's.
async fn send(pool: &PgPool, s: &Seed, source: &str, fixture: &str) {
    let payload: Value = serde_json::from_str(fixture).unwrap();
    let external_id = payload["id"]
        .as_str()
        .or(payload["event_id"].as_str())
        .unwrap()
        .to_owned();
    insert_events(
        pool,
        &[InboundEvent {
            id: Uuid::now_v7(),
            program_id: s.program,
            source: source.into(),
            external_id,
            payload,
            received_ms: Utc::now().timestamp_millis(),
        }],
    )
    .await
    .unwrap();
    process_events(pool).await.unwrap();
}

async fn status(pool: &PgPool, s: &Seed) -> String {
    sqlx::query_scalar("SELECT status::text FROM conversions WHERE program_id = $1")
        .bind(s.program)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn balance(pool: &PgPool, s: &Seed) -> (i64, i64) {
    let b = bf_db::ledger::balance_for_affiliate(pool, s.program, s.aff)
        .await
        .unwrap();
    (b.pending_cents, b.payable_cents)
}

async fn assert_balanced(pool: &PgPool) {
    let bad: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT txn_id FROM ledger_entries GROUP BY txn_id HAVING sum(amount_cents) <> 0) t",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(bad, 0);
}

async fn approved(pool: &PgPool) -> Seed {
    let s = seed(pool, true).await;
    send(pool, &s, "stripe", STRIPE_SALE).await;
    assert_eq!(approve_due(pool).await.unwrap(), 1);
    assert_eq!(
        (status(pool, &s).await.as_str(), balance(pool, &s).await),
        ("approved", (0, 1_000))
    );
    s
}

#[sqlx::test(migrations = "../db/migrations")]
async fn chargeback_webhook_stripe_dispute_reverses_approved_conversion(pool: PgPool) {
    let s = approved(&pool).await;
    // The dispute names the charge `ch_1`; its event id differs from the sale's `evt_sale_1`.
    send(&pool, &s, "stripe", STRIPE_DISPUTE).await;
    assert_eq!(status(&pool, &s).await, "reversed");
    assert_eq!(balance(&pool, &s).await, (0, 0));
    // AffPayable debited, Expense credited by the commission.
    let legs: Vec<(String, i64)> = sqlx::query_as(
        "SELECT l.account::text, l.amount_cents FROM ledger_entries l \
         WHERE l.txn_id = (SELECT txn_id FROM ledger_entries ORDER BY id DESC LIMIT 1)",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(legs.contains(&("aff_payable".into(), 1_000)), "{legs:?}");
    assert!(legs.contains(&("expense".into(), -1_000)), "{legs:?}");
    assert_balanced(&pool).await;
    // Replaying the same dispute (or a refund after it) is a no-op.
    send(&pool, &s, "stripe", STRIPE_REFUND).await;
    assert_eq!(balance(&pool, &s).await, (0, 0));
    assert_balanced(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn chargeback_webhook_pending_conversion_is_rejected_not_reversed(pool: PgPool) {
    let s = seed(&pool, false).await;
    send(&pool, &s, "stripe", STRIPE_SALE).await;
    assert_eq!(status(&pool, &s).await, "pending");
    send(&pool, &s, "stripe", STRIPE_DISPUTE).await;
    assert_eq!(status(&pool, &s).await, "rejected");
    assert_eq!(balance(&pool, &s).await, (0, 0));
    assert_balanced(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn chargeback_webhook_stripe_refund_matches_by_charge_not_event_id(pool: PgPool) {
    let s = approved(&pool).await;
    send(&pool, &s, "stripe", STRIPE_REFUND).await;
    assert_eq!(status(&pool, &s).await, "reversed");
    assert_eq!(balance(&pool, &s).await, (0, 0));
}

async fn paddle_case(pool: &PgPool, fixture: &str) {
    let s = seed(pool, true).await;
    send(pool, &s, "paddle", PADDLE_SALE).await;
    assert_eq!(approve_due(pool).await.unwrap(), 1);
    assert_eq!(balance(pool, &s).await, (0, 2_000));
    send(pool, &s, "paddle", fixture).await;
    assert_eq!(status(pool, &s).await, "reversed");
    assert_eq!(balance(pool, &s).await, (0, 0));
    assert_balanced(pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn chargeback_webhook_paddle_refund_reverses_by_transaction_id(pool: PgPool) {
    paddle_case(&pool, PADDLE_REFUND).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn chargeback_webhook_paddle_chargeback_reverses_by_transaction_id(pool: PgPool) {
    paddle_case(&pool, PADDLE_CHARGEBACK).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn chargeback_webhook_unknown_charge_does_not_wedge_the_queue(pool: PgPool) {
    let s = seed(&pool, true).await;
    send(&pool, &s, "stripe", STRIPE_DISPUTE).await; // no sale was ever ingested
    // Still queued for retry, and later events in the same tick are processed.
    send(&pool, &s, "stripe", STRIPE_SALE).await;
    let sales: i64 = sqlx::query_scalar("SELECT count(*) FROM conversions WHERE program_id = $1")
        .bind(s.program)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(sales, 1);
}
