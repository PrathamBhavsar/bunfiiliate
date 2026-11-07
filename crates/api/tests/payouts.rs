use api::error::AppError;
use api::payouts::{approve_batch, batch_csv, cancel_batch, create_batch, mark_paid};
use api::worker::{approve_due, process_events};
use bf_db::ledger::balance_for_affiliate;
use bf_db::tracker::{InboundEvent, insert_events};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// A program (USD, 10% lifetime, manual or auto approve) with two active affiliates.
struct Seed {
    workspace: Uuid,
    program: Uuid,
    admin: Uuid,
    a: Uuid,
    b: Uuid,
}

async fn user(pool: &PgPool, workspace: Uuid, role: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO users (id, workspace_id, email, password_hash, role) \
         VALUES ($1, $2, $3, 'h', $4::user_role)",
    )
    .bind(id)
    .bind(workspace)
    .bind(format!("{id}@x.io"))
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn affiliate(pool: &PgPool, workspace: Uuid, program: Uuid, code: &str, email: &str) -> Uuid {
    let (user, aff) = (user(pool, workspace, "affiliate").await, Uuid::now_v7());
    sqlx::query(
        "INSERT INTO affiliates (id, workspace_id, user_id, status, payout_email) \
         VALUES ($1, $2, $3, 'active', $4)",
    )
    .bind(aff)
    .bind(workspace)
    .bind(user)
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO enrollments (affiliate_id, program_id) VALUES ($1, $2)")
        .bind(aff)
        .bind(program)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO referral_codes (code, affiliate_id, program_id, landing_url) \
         VALUES ($1, $2, $3, 'https://x.io/')",
    )
    .bind(code)
    .bind(aff)
    .bind(program)
    .execute(pool)
    .await
    .unwrap();
    aff
}

async fn seed(pool: &PgPool, auto_approve: bool) -> Seed {
    let (workspace, program, plan) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', $2)")
        .bind(workspace)
        .bind(workspace.to_string())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, auto_approve, hold_days, \
           public_key, webhook_secret) VALUES ($1, $2, 'p', 'USD', $3, 0, $4, 's')",
    )
    .bind(program)
    .bind(workspace)
    .bind(auto_approve)
    .bind(program.to_string())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commission_plans (id, program_id, kind, rate_bps, duration) \
         VALUES ($1, $2, 'percent', 1000, 'lifetime')",
    )
    .bind(plan)
    .bind(program)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE programs SET default_plan_id = $2 WHERE id = $1")
        .bind(program)
        .bind(plan)
        .execute(pool)
        .await
        .unwrap();
    Seed {
        workspace,
        program,
        admin: user(pool, workspace, "admin").await,
        a: affiliate(pool, workspace, program, &format!("A{program}"), "a@x.io").await,
        b: affiliate(pool, workspace, program, &format!("B{program}"), "b@x.io").await,
    }
}

async fn send(pool: &PgPool, s: &Seed, source: &str, external_id: &str, payload: Value) {
    let n = insert_events(
        pool,
        &[InboundEvent {
            id: Uuid::now_v7(),
            program_id: s.program,
            source: source.into(),
            external_id: external_id.into(),
            payload,
            received_ms: Utc::now().timestamp_millis(),
        }],
    )
    .await
    .unwrap();
    assert_eq!(n, 1);
}

/// A sale credited to `code`'s affiliate; the order id doubles as customer and external id.
async fn sale(pool: &PgPool, s: &Seed, order: &str, cents: i64, aff: Uuid) {
    let code = format!("{}{}", if aff == s.a { "A" } else { "B" }, s.program);
    send(
        pool,
        s,
        "generic",
        order,
        json!({"customer": order, "amount_cents": cents, "code": code,
               "occurred_at": (Utc::now() - Duration::hours(1)).to_rfc3339()}),
    )
    .await;
    process_events(pool).await.unwrap();
}

async fn reverse(pool: &PgPool, s: &Seed, kind: &str, order: &str) {
    send(
        pool,
        s,
        "generic",
        &format!("{kind}-{order}"),
        json!({"type": kind, "conversion": order}),
    )
    .await;
    process_events(pool).await.unwrap();
}

async fn status_of(pool: &PgPool, order: &str) -> String {
    sqlx::query_scalar("SELECT status::text FROM conversions WHERE external_id = $1")
        .bind(order)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn balance(pool: &PgPool, s: &Seed, aff: Uuid) -> (i64, i64) {
    let b = balance_for_affiliate(pool, s.program, aff).await.unwrap();
    (b.pending_cents, b.payable_cents)
}

async fn ledger_ids(pool: &PgPool) -> Vec<Uuid> {
    sqlx::query_scalar("SELECT id FROM ledger_entries ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Every txn balances and `payable + pending == live commissions - paid out`.
async fn assert_books(pool: &PgPool) {
    let unbalanced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT txn_id FROM ledger_entries GROUP BY txn_id \
         HAVING sum(amount_cents) <> 0) t",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(unbalanced, 0);
    let (owed, live, paid): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT COALESCE(sum(pending_cents + payable_cents), 0) FROM affiliate_balances)::bigint, \
           (SELECT COALESCE(sum(commission_cents), 0) FROM conversions \
              WHERE status IN ('pending', 'approved', 'paid'))::bigint, \
           (SELECT COALESCE(sum(amount_cents), 0) FROM payouts WHERE status = 'paid')::bigint",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(owed, live - paid);
}

async fn pay(pool: &PgPool, s: &Seed) -> Uuid {
    let batch = create_batch(pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    approve_batch(pool, s.workspace, batch, s.admin)
        .await
        .unwrap();
    mark_paid(pool, s.workspace, batch).await.unwrap();
    batch
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_batch_respects_minimum_and_never_double_claims(pool: PgPool) {
    let s = seed(&pool, true).await;
    sqlx::query("UPDATE affiliates SET payout_min_cents = 5000 WHERE id = $1")
        .bind(s.b)
        .execute(&pool)
        .await
        .unwrap();
    sale(&pool, &s, "o1", 10_000, s.a).await; // 1000 each
    sale(&pool, &s, "o2", 10_000, s.b).await;
    assert_eq!(
        create_batch(&pool, s.workspace, s.program, s.admin)
            .await
            .unwrap(),
        None
    ); // none approved yet
    approve_due(&pool).await.unwrap();

    let batch = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    let rows: Vec<(Uuid, i64, String)> = sqlx::query_as(
        "SELECT affiliate_id, amount_cents, status::text FROM payouts WHERE batch_id = $1",
    )
    .bind(batch)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, [(s.a, 1_000, "draft".into())]);
    let claimed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM conversions WHERE payout_id IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(claimed, 1);
    // A's payable is already in the open batch.
    assert_eq!(
        create_batch(&pool, s.workspace, s.program, s.admin)
            .await
            .unwrap(),
        None
    );

    // B reaches its minimum with a second sale; A earns a little more.
    sale(&pool, &s, "o3", 40_000, s.b).await; // +4000 -> 5000
    sale(&pool, &s, "o4", 1_000, s.a).await; // +100
    approve_due(&pool).await.unwrap();
    let second = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT affiliate_id, amount_cents FROM payouts WHERE batch_id = $1 ORDER BY amount_cents",
    )
    .bind(second)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, [(s.a, 100), (s.b, 5_000)]);
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_paid_flow_balances_and_double_pay_is_rejected(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    sale(&pool, &s, "o2", 25_000, s.a).await;
    sale(&pool, &s, "o3", 5_000, s.b).await;
    approve_due(&pool).await.unwrap();

    let batch = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    // A draft batch cannot be paid.
    assert!(matches!(
        mark_paid(&pool, s.workspace, batch).await,
        Err(AppError::Conflict(_))
    ));
    approve_batch(&pool, s.workspace, batch, s.admin)
        .await
        .unwrap();
    assert!(matches!(
        approve_batch(&pool, s.workspace, batch, s.admin).await,
        Err(AppError::Conflict(_))
    ));
    assert!(matches!(
        cancel_batch(&pool, s.workspace, batch).await,
        Err(AppError::Conflict(_))
    ));
    assert_books(&pool).await;

    mark_paid(&pool, s.workspace, batch).await.unwrap();
    assert_eq!(balance(&pool, &s, s.a).await, (0, 0));
    assert_eq!(balance(&pool, &s, s.b).await, (0, 0));
    for order in ["o1", "o2", "o3"] {
        assert_eq!(status_of(&pool, order).await, "paid");
    }
    let (status, paid_at): (String, bool) = sqlx::query_as(
        "SELECT status::text, paid_at IS NOT NULL FROM payout_batches WHERE id = $1",
    )
    .bind(batch)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((status.as_str(), paid_at), ("paid", true));
    let cash: i64 = sqlx::query_scalar(
        "SELECT -sum(amount_cents)::bigint FROM ledger_entries WHERE account = 'cash'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cash, 3_500 + 500);
    let linked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payouts p JOIN ledger_entries l ON l.txn_id = p.ledger_txn_id \
         WHERE p.status = 'paid'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, 4); // two entries per payout
    assert_books(&pool).await;

    let before = ledger_ids(&pool).await;
    assert!(matches!(
        mark_paid(&pool, s.workspace, batch).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(ledger_ids(&pool).await, before);
    assert!(matches!(
        mark_paid(&pool, s.workspace, Uuid::now_v7()).await,
        Err(AppError::NotFound(_))
    ));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_refund_before_approval_rejects_and_replays_safely(pool: PgPool) {
    let s = seed(&pool, false).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    sale(&pool, &s, "o2", 10_000, s.a).await;
    assert_eq!(balance(&pool, &s, s.a).await, (2_000, 0));

    reverse(&pool, &s, "refund", "o1").await;
    assert_eq!(status_of(&pool, "o1").await, "rejected");
    assert_eq!(status_of(&pool, "o2").await, "pending");
    assert_eq!(balance(&pool, &s, s.a).await, (1_000, 0));
    // A chargeback for the same conversion, and one for an unknown order, change nothing.
    let before = ledger_ids(&pool).await;
    reverse(&pool, &s, "chargeback", "o1").await;
    reverse(&pool, &s, "refund", "nope").await;
    assert_eq!(ledger_ids(&pool).await, before);
    let errors: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inbound_events WHERE error IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(errors, 0);
    assert_eq!(approve_due(&pool).await.unwrap(), 0); // manual program
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_chargeback_after_payout_nets_the_next_batch(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    approve_due(&pool).await.unwrap();
    pay(&pool, &s).await;
    assert_eq!(status_of(&pool, "o1").await, "paid");
    let past = ledger_ids(&pool).await;

    reverse(&pool, &s, "chargeback", "o1").await;
    assert_eq!(status_of(&pool, "o1").await, "reversed");
    assert_eq!(balance(&pool, &s, s.a).await, (0, -1_000));
    // Only appended: every earlier row is still there.
    let after = ledger_ids(&pool).await;
    assert_eq!(after[..past.len()], past[..]);
    assert_eq!(after.len(), past.len() + 2);
    assert_books(&pool).await;
    // Nothing is owed while the affiliate is in debt.
    assert_eq!(
        create_batch(&pool, s.workspace, s.program, s.admin)
            .await
            .unwrap(),
        None
    );

    sale(&pool, &s, "o2", 15_000, s.a).await; // 1500 - 1000 clawback
    approve_due(&pool).await.unwrap();
    assert_eq!(balance(&pool, &s, s.a).await, (0, 500));
    let batch = pay(&pool, &s).await;
    let amount: i64 = sqlx::query_scalar("SELECT amount_cents FROM payouts WHERE batch_id = $1")
        .bind(batch)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(amount, 500);
    assert_eq!(balance(&pool, &s, s.a).await, (0, 0));
    assert_eq!(status_of(&pool, "o2").await, "paid");
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_reversal_inside_an_open_batch_shrinks_the_payout(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    sale(&pool, &s, "o2", 30_000, s.a).await;
    approve_due(&pool).await.unwrap();
    let batch = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    approve_batch(&pool, s.workspace, batch, s.admin)
        .await
        .unwrap();

    reverse(&pool, &s, "refund", "o2").await; // 3000 of the 4000
    mark_paid(&pool, s.workspace, batch).await.unwrap();
    let amount: i64 = sqlx::query_scalar("SELECT amount_cents FROM payouts WHERE batch_id = $1")
        .bind(batch)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(amount, 1_000);
    assert_eq!(status_of(&pool, "o1").await, "paid");
    assert_eq!(status_of(&pool, "o2").await, "reversed");
    assert_eq!(balance(&pool, &s, s.a).await, (0, 0));
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_provider_refunds_find_the_original_conversion(pool: PgPool) {
    let s = seed(&pool, false).await;
    let code = format!("A{}", s.program);
    send(
        &pool,
        &s,
        "stripe",
        "evt_1",
        json!({"id": "evt_1", "type": "invoice.paid", "created": Utc::now().timestamp() - 60,
               "data": {"object": {"id": "in_1", "customer": "cus_1", "charge": "ch_1",
                                   "payment_intent": "pi_1", "amount_paid": 10_000,
                                   "currency": "usd", "metadata": {"bf_ref": code}}}}),
    )
    .await;
    send(
        &pool,
        &s,
        "paddle",
        "evt_2",
        json!({"event_id": "evt_2", "event_type": "transaction.completed",
               "occurred_at": (Utc::now() - Duration::minutes(5)).to_rfc3339(),
               "data": {"id": "txn_1", "customer_id": "ctm_1", "currency_code": "USD",
                        "details": {"totals": {"total": "20000"}},
                        "custom_data": {"bf_ref": code}}}),
    )
    .await;
    process_events(&pool).await.unwrap();
    assert_eq!(balance(&pool, &s, s.a).await, (3_000, 0));

    send(
        &pool,
        &s,
        "stripe",
        "evt_3",
        json!({"id": "evt_3", "type": "charge.refunded",
               "data": {"object": {"id": "ch_1", "invoice": "in_1", "payment_intent": "pi_1"}}}),
    )
    .await;
    send(
        &pool,
        &s,
        "paddle",
        "evt_4",
        json!({"event_id": "evt_4", "event_type": "adjustment.created",
               "data": {"action": "chargeback", "transaction_id": "txn_1"}}),
    )
    .await;
    process_events(&pool).await.unwrap();
    assert_eq!(status_of(&pool, "evt_1").await, "rejected");
    assert_eq!(status_of(&pool, "evt_2").await, "rejected");
    assert_eq!(balance(&pool, &s, s.a).await, (0, 0));
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_cancelled_batch_releases_its_conversions(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    approve_due(&pool).await.unwrap();
    let first = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    cancel_batch(&pool, s.workspace, first).await.unwrap();
    assert!(matches!(
        approve_batch(&pool, s.workspace, first, s.admin).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(balance(&pool, &s, s.a).await, (0, 1_000));

    let second = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first, second);
    approve_batch(&pool, s.workspace, second, s.admin)
        .await
        .unwrap();
    mark_paid(&pool, s.workspace, second).await.unwrap();
    assert_eq!(status_of(&pool, "o1").await, "paid");
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_csv_export_is_exact(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 12_345, s.a).await; // 10% of 12345 = 1234.5, rounds half up to 1235
    sale(&pool, &s, "o2", 5_000, s.b).await;
    approve_due(&pool).await.unwrap();
    let batch = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        batch_csv(&pool, s.workspace, batch).await.unwrap(),
        "email,amount,currency\na@x.io,12.35,USD\nb@x.io,5.00,USD\n"
    );
    assert!(matches!(
        batch_csv(&pool, s.workspace, Uuid::now_v7()).await,
        Err(AppError::NotFound(_))
    ));
}

// ---- review fixes

/// A Stripe invoice/checkout pair for one payment: same invoice, different events.
fn stripe_invoice(evt: &str, invoice: &str, code: &str) -> Value {
    json!({"id": evt, "type": "invoice.paid", "created": Utc::now().timestamp() - 60,
           "data": {"object": {"id": invoice, "customer": "cus_9", "payment_intent": "pi_9",
                               "amount_paid": 4_900, "currency": "usd",
                               "metadata": {"bf_ref": code}}}})
}

fn stripe_checkout(evt: &str, invoice: &str, code: &str, payment_status: &str) -> Value {
    json!({"id": evt, "type": "checkout.session.completed", "created": Utc::now().timestamp() - 60,
           "data": {"object": {"id": "cs_9", "customer": "cus_9", "invoice": invoice,
                               "payment_status": payment_status, "amount_total": 4_900,
                               "currency": "usd", "metadata": {"bf_ref": code}}}})
}

async fn conversions(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM conversions")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_clawback_then_refund_in_open_batch_stays_payable(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    approve_due(&pool).await.unwrap();
    pay(&pool, &s).await;
    reverse(&pool, &s, "chargeback", "o1").await; // payable -1000
    sale(&pool, &s, "o2", 15_000, s.a).await; // +1500 -> 500
    approve_due(&pool).await.unwrap();
    let batch = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    approve_batch(&pool, s.workspace, batch, s.admin)
        .await
        .unwrap();

    // The 500 payout claimed a 1500 commission; refunding it must not go below zero.
    reverse(&pool, &s, "refund", "o2").await;
    let amount: i64 = sqlx::query_scalar("SELECT amount_cents FROM payouts WHERE batch_id = $1")
        .bind(batch)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(amount, 0, "clamped, not -1000");
    mark_paid(&pool, s.workspace, batch)
        .await
        .expect("batch is still payable");
    assert_eq!(status_of(&pool, "o2").await, "reversed");
    assert_eq!(
        batch_csv(&pool, s.workspace, batch).await.unwrap(),
        "email,amount,currency\n"
    );
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_stripe_invoice_and_checkout_pay_one_commission(pool: PgPool) {
    let s = seed(&pool, false).await;
    let code = format!("A{}", s.program);
    send(
        &pool,
        &s,
        "stripe",
        "evt_a",
        stripe_checkout("evt_a", "in_9", &code, "paid"),
    )
    .await;
    send(
        &pool,
        &s,
        "stripe",
        "evt_b",
        stripe_invoice("evt_b", "in_9", &code),
    )
    .await;
    process_events(&pool).await.unwrap();
    assert_eq!(conversions(&pool).await, 1, "one payment, one conversion");
    assert_eq!(balance(&pool, &s, s.a).await, (490, 0));

    // The refund names the invoice and reverses it entirely.
    send(
        &pool,
        &s,
        "stripe",
        "evt_r",
        json!({"id": "evt_r", "type": "charge.refunded",
               "data": {"object": {"id": "ch_9", "invoice": "in_9", "refunded": true}}}),
    )
    .await;
    process_events(&pool).await.unwrap();
    assert_eq!(balance(&pool, &s, s.a).await, (0, 0));
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_unpaid_checkout_pays_nothing_until_async_payment_succeeds(pool: PgPool) {
    let s = seed(&pool, false).await;
    let code = format!("A{}", s.program);
    send(
        &pool,
        &s,
        "stripe",
        "evt_1",
        stripe_checkout("evt_1", "in_8", &code, "unpaid"),
    )
    .await;
    process_events(&pool).await.unwrap();
    assert_eq!(conversions(&pool).await, 0, "no money yet");

    let mut ok = stripe_checkout("evt_2", "in_8", &code, "paid");
    ok["type"] = json!("checkout.session.async_payment_succeeded");
    send(&pool, &s, "stripe", "evt_2", ok).await;
    process_events(&pool).await.unwrap();
    assert_eq!(conversions(&pool).await, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_refund_before_its_sale_waits_then_reverses(pool: PgPool) {
    let s = seed(&pool, false).await;
    let code = format!("A{}", s.program);
    let refund = json!({"id": "evt_r", "type": "charge.refunded",
                        "data": {"object": {"id": "ch_9", "invoice": "in_7", "refunded": true}}});
    send(&pool, &s, "stripe", "evt_r", refund).await;
    process_events(&pool).await.unwrap();
    let (processed, attempts): (bool, i32) = sqlx::query_as(
        "SELECT processed_at IS NOT NULL, attempts FROM inbound_events WHERE external_id = 'evt_r'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!processed, "kept for a retry, not silently dropped");
    assert_eq!(attempts, 1);

    // The sale arrives late; the retry (once due) finds it.
    send(
        &pool,
        &s,
        "stripe",
        "evt_s",
        stripe_invoice("evt_s", "in_7", &code),
    )
    .await;
    sqlx::query("UPDATE inbound_events SET retry_at = now() WHERE external_id = 'evt_r'")
        .execute(&pool)
        .await
        .unwrap();
    process_events(&pool).await.unwrap(); // refund still has no sale, backs off again
    sqlx::query("UPDATE inbound_events SET retry_at = now() WHERE external_id = 'evt_r'")
        .execute(&pool)
        .await
        .unwrap();
    process_events(&pool).await.unwrap();
    assert_eq!(status_of(&pool, "evt_s").await, "rejected");
    assert_eq!(balance(&pool, &s, s.a).await, (0, 0));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_unmatched_refund_gives_up_after_a_few_tries(pool: PgPool) {
    let s = seed(&pool, false).await;
    let refund = json!({"id": "evt_r", "type": "charge.refunded",
                        "data": {"object": {"id": "ch_x", "invoice": "in_none", "refunded": true}}});
    send(&pool, &s, "stripe", "evt_r", refund).await;
    for _ in 0..8 {
        sqlx::query("UPDATE inbound_events SET retry_at = now() WHERE processed_at IS NULL")
            .execute(&pool)
            .await
            .unwrap();
        process_events(&pool).await.unwrap();
    }
    let (processed, error): (bool, Option<String>) = sqlx::query_as(
        "SELECT processed_at IS NOT NULL, error FROM inbound_events WHERE external_id = 'evt_r'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(processed, "the queue is not blocked by an organic refund");
    assert_eq!(error, None);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_partial_refunds_do_not_claw_back(pool: PgPool) {
    let s = seed(&pool, false).await;
    let code = format!("A{}", s.program);
    send(
        &pool,
        &s,
        "stripe",
        "evt_s",
        stripe_invoice("evt_s", "in_6", &code),
    )
    .await;
    send(
        &pool,
        &s,
        "paddle",
        "evt_p",
        json!({"event_id": "evt_p", "event_type": "transaction.completed",
               "occurred_at": (Utc::now() - Duration::minutes(5)).to_rfc3339(),
               "data": {"id": "txn_6", "customer_id": "ctm_6", "currency_code": "USD",
                        "details": {"totals": {"total": "20000"}},
                        "custom_data": {"bf_ref": code}}}),
    )
    .await;
    process_events(&pool).await.unwrap();
    assert_eq!(balance(&pool, &s, s.a).await, (2_490, 0));

    send(
        &pool,
        &s,
        "stripe",
        "evt_r",
        json!({"id": "evt_r", "type": "charge.refunded",
               "data": {"object": {"id": "ch_6", "invoice": "in_6", "refunded": false,
                                   "amount_refunded": 100}}}),
    )
    .await;
    send(
        &pool,
        &s,
        "paddle",
        "evt_a",
        json!({"event_id": "evt_a", "event_type": "adjustment.created",
               "data": {"action": "refund", "type": "partial", "transaction_id": "txn_6"}}),
    )
    .await;
    process_events(&pool).await.unwrap();
    assert_eq!(
        balance(&pool, &s, s.a).await,
        (2_490, 0),
        "commissions intact"
    );
    assert_eq!(status_of(&pool, "evt_s").await, "pending");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_zero_amount_sale_earns_no_commission(pool: PgPool) {
    let s = seed(&pool, false).await;
    sqlx::query("UPDATE commission_plans SET kind = 'flat', rate_bps = NULL, amount_cents = 500")
        .execute(&pool)
        .await
        .unwrap();
    sale(&pool, &s, "trial", 0, s.a).await;
    assert_eq!(conversions(&pool).await, 0, "a $0 invoice is not a sale");
    sale(&pool, &s, "real", 1_000, s.a).await;
    assert_eq!(conversions(&pool).await, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_email_is_snapshotted_when_the_batch_is_drafted(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    approve_due(&pool).await.unwrap();
    let batch = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    approve_batch(&pool, s.workspace, batch, s.admin)
        .await
        .unwrap();

    sqlx::query("UPDATE affiliates SET payout_email = 'attacker@evil.io' WHERE id = $1")
        .bind(s.a)
        .execute(&pool)
        .await
        .unwrap();
    let csv = batch_csv(&pool, s.workspace, batch).await.unwrap();
    assert!(csv.contains("a@x.io,10.00,USD"), "{csv}");
    assert!(!csv.contains("evil"), "{csv}");
}

// ---- C04: db/api fns refuse a foreign workspace even when the handler check is skipped

async fn other_workspace(pool: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'o', $2)")
        .bind(id)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn batch_status(pool: &PgPool, batch: Uuid) -> String {
    sqlx::query_scalar("SELECT status::text FROM payout_batches WHERE id = $1")
        .bind(batch)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn leak_payout_create_batch_1(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    approve_due(&pool).await.unwrap();
    let other = other_workspace(&pool).await;
    assert!(matches!(
        create_batch(&pool, other, s.program, s.admin).await,
        Err(AppError::NotFound(_))
    ));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM payout_batches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn leak_payout_batch_actions_2(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    approve_due(&pool).await.unwrap();
    let batch = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    let other = other_workspace(&pool).await;
    assert!(matches!(
        approve_batch(&pool, other, batch, s.admin).await,
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        cancel_batch(&pool, other, batch).await,
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        mark_paid(&pool, other, batch).await,
        Err(AppError::NotFound(_))
    ));
    assert_eq!(batch_status(&pool, batch).await, "draft");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn leak_payout_batch_csv_3(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    approve_due(&pool).await.unwrap();
    let batch = create_batch(&pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    let other = other_workspace(&pool).await;
    assert!(matches!(
        batch_csv(&pool, other, batch).await,
        Err(AppError::NotFound(_))
    ));
    let mut conn = pool.acquire().await.unwrap();
    assert!(
        bf_db::payouts::batch_rows(&mut conn, other, batch)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        bf_db::payouts::owed(&mut conn, other, s.program)
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn leak_db_code_and_delivery_fns_4(pool: PgPool) {
    let s = seed(&pool, true).await;
    let other = other_workspace(&pool).await;
    // count_codes: a foreign workspace sees zero codes for this affiliate.
    assert_eq!(
        bf_db::affiliates::count_codes(&pool, other, s.a)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        bf_db::affiliates::count_codes(&pool, s.workspace, s.a)
            .await
            .unwrap(),
        1
    );
    // delete_offsite_codes: the foreign workspace deletes nothing (the seed code is offsite: no sites).
    assert_eq!(
        bf_db::programs::delete_offsite_codes(&pool, other, s.program)
            .await
            .unwrap(),
        0
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM referral_codes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
}

// ---- payout rails (B04)

use api::payout_rails::{MockRail, RailClient, dispatch, load_client, reconcile};
use bf_core::Rail;
use bf_core::secrets::Vault;
use std::collections::HashMap;
use std::sync::Arc;

/// Two affiliates owed 1000 each, batch drafted and approved but not paid.
async fn approved_batch(pool: &PgPool) -> (Seed, Uuid) {
    let s = seed(pool, true).await;
    sale(pool, &s, "o1", 10_000, s.a).await;
    sale(pool, &s, "o2", 10_000, s.b).await;
    approve_due(pool).await.unwrap();
    let batch = create_batch(pool, s.workspace, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    approve_batch(pool, s.workspace, batch, s.admin)
        .await
        .unwrap();
    (s, batch)
}

fn mock() -> (Arc<MockRail>, RailClient) {
    let m = Arc::new(MockRail::default());
    (m.clone(), RailClient::Mock(m))
}

async fn line_states(pool: &PgPool, batch: Uuid) -> Vec<(String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT COALESCE(payout_email, ''), status::text, rail_state FROM payouts \
         WHERE batch_id = $1 ORDER BY payout_email",
    )
    .bind(batch)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_manual_csv_leaves_rail_columns_empty(pool: PgPool) {
    let (s, batch) = approved_batch(&pool).await;
    mark_paid(&pool, s.workspace, batch).await.unwrap();
    let rows = line_states(&pool, batch).await;
    assert!(rows.iter().all(|r| r.1 == "paid" && r.2.is_none()));
    let rails: Vec<String> = sqlx::query_scalar("SELECT DISTINCT rail FROM payouts")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rails, ["manual_csv"]);
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_mock_pays_each_line_once_and_posts_ledger(pool: PgPool) {
    let (s, batch) = approved_batch(&pool).await;
    let (m, client) = mock();
    m.probe(pool.clone());
    let out = dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
        .await
        .unwrap();
    assert_eq!((out.paid, out.failed, out.submitted), (2, 0, 0));
    assert!(out.batch_paid);
    assert_eq!(batch_status(&pool, batch).await, "paid");
    assert_eq!((m.accepted(), m.total_cents()), (2, 2_000));
    let rows = line_states(&pool, batch).await;
    assert!(
        rows.iter()
            .all(|r| r.1 == "paid" && r.2.as_deref() == Some("paid"))
    );
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payouts WHERE rail_receipt IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(receipts, 2);
    assert_eq!(balance(&pool, &s, s.a).await, (0, 0));
    assert_books(&pool).await;
    // A paid batch cannot be sent again.
    assert!(
        dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
            .await
            .is_err()
    );
    assert_eq!(m.accepted(), 2);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_provider_call_runs_outside_any_db_transaction(pool: PgPool) {
    let (s, batch) = approved_batch(&pool).await;
    let (m, client) = mock();
    m.probe(pool.clone());
    dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
        .await
        .unwrap();
    assert_eq!(m.calls(), 2);
    assert_eq!(m.idle_in_tx_seen(), 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_provider_failure_mid_batch_then_retry_never_double_pays(pool: PgPool) {
    let (s, batch) = approved_batch(&pool).await;
    let (m, client) = mock();
    m.reject("a@x.io");
    let out = dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
        .await
        .unwrap();
    // a fails, b (processed after the failure) still goes out.
    assert_eq!((out.paid, out.failed), (1, 1));
    assert!(!out.batch_paid);
    assert_eq!(batch_status(&pool, batch).await, "approved");
    let rows = line_states(&pool, batch).await;
    assert_eq!(
        rows,
        [
            ("a@x.io".into(), "approved".into(), Some("failed".into())),
            ("b@x.io".into(), "paid".into(), Some("paid".into())),
        ]
    );
    // Only the confirmed line hit the ledger.
    assert_eq!(balance(&pool, &s, s.a).await, (0, 1_000));
    assert_eq!(balance(&pool, &s, s.b).await, (0, 0));
    let err: Option<String> =
        sqlx::query_scalar("SELECT rail_error FROM payouts WHERE payout_email = 'a@x.io'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(err.unwrap().contains("refused"));
    assert_books(&pool).await;

    // Manual pay is allowed for a failed line only once nothing is in flight; here retry the rail.
    m.heal("a@x.io");
    let out = dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
        .await
        .unwrap();
    assert_eq!((out.paid, out.failed), (1, 0));
    assert!(out.batch_paid);
    // Two lines, two keys, 2000 total: b was not sent again.
    assert_eq!((m.accepted(), m.total_cents()), (2, 2_000));
    assert_eq!(m.calls(), 4 - 1);
    assert_eq!(balance(&pool, &s, s.a).await, (0, 0));
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_timeout_is_retried_with_the_same_key(pool: PgPool) {
    let (s, batch) = approved_batch(&pool).await;
    let (m, client) = mock();
    m.transient("a@x.io");
    let out = dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
        .await
        .unwrap();
    assert_eq!((out.paid, out.submitted), (1, 1));
    // Unknown outcome: stays submitted, no receipt, nothing posted for it.
    let rows = line_states(&pool, batch).await;
    assert_eq!(rows[0].2.as_deref(), Some("submitted"));
    assert_eq!(rows[0].1, "approved");
    assert_eq!(balance(&pool, &s, s.a).await, (0, 1_000));
    // Pay-by-hand is refused while the provider may have the money.
    assert!(matches!(
        mark_paid(&pool, s.workspace, batch).await,
        Err(AppError::Conflict(_))
    ));
    m.heal("a@x.io");
    let out = dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
        .await
        .unwrap();
    assert_eq!(out.paid, 1);
    assert!(out.batch_paid);
    assert_eq!(m.accepted(), 2);
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_pending_posts_ledger_only_after_reconcile_confirms(pool: PgPool) {
    let (s, batch) = approved_batch(&pool).await;
    let (m, client) = mock();
    m.hold("a@x.io");
    let out = dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
        .await
        .unwrap();
    assert_eq!((out.paid, out.submitted), (1, 1));
    assert_eq!(balance(&pool, &s, s.a).await, (0, 1_000)); // not paid yet
    let st: (String, Option<String>) =
        sqlx::query_as("SELECT rail_state, rail_status FROM payouts WHERE payout_email = 'a@x.io'")
            .fetch_one(&pool)
            .await
            .map(|r: (Option<String>, Option<String>)| (r.0.unwrap(), r.1))
            .unwrap();
    assert_eq!(st, ("submitted".into(), Some("pending".into())));

    // Re-dispatch does not send the held line again.
    dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
        .await
        .unwrap();
    assert_eq!(m.calls(), 2);

    let clients = HashMap::from([(Rail::Mock, client.clone())]);
    let out = reconcile(&pool, s.workspace, batch, &clients)
        .await
        .unwrap();
    assert_eq!((out.paid, out.submitted, out.batch_paid), (0, 1, false));
    assert_eq!(balance(&pool, &s, s.a).await, (0, 1_000));

    m.release();
    let out = reconcile(&pool, s.workspace, batch, &clients)
        .await
        .unwrap();
    assert_eq!((out.paid, out.batch_paid), (1, true));
    assert_eq!(balance(&pool, &s, s.a).await, (0, 0));
    assert_eq!(batch_status(&pool, batch).await, "paid");
    assert_books(&pool).await;
    // Reconciling again changes nothing.
    let before = ledger_ids(&pool).await;
    reconcile(&pool, s.workspace, batch, &clients)
        .await
        .unwrap();
    assert_eq!(ledger_ids(&pool).await, before);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_unconfigured_provider_is_422_and_touches_nothing(pool: PgPool) {
    let (s, batch) = approved_batch(&pool).await;
    for rail in [Rail::PaypalPayouts, Rail::Wise] {
        let err = load_client(&pool, &Vault::plaintext(), s.workspace, rail)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, AppError::Unprocessable(_)));
    }
    assert_eq!(batch_status(&pool, batch).await, "approved");
    assert!(
        line_states(&pool, batch)
            .await
            .iter()
            .all(|r| r.1 == "approved" && r.2.is_none())
    );
    assert_eq!(balance(&pool, &s, s.a).await, (0, 1_000));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payout_rail_zero_amount_line_is_closed_without_a_provider_call(pool: PgPool) {
    let (s, batch) = approved_batch(&pool).await;
    sale(&pool, &s, "o3", 5_000, s.b).await; // unrelated; keeps b earning
    reverse(&pool, &s, "refund", "o2").await; // shrinks b's open payout to 0
    let (m, client) = mock();
    let out = dispatch(&pool, s.workspace, batch, Rail::Mock, &client)
        .await
        .unwrap();
    assert_eq!(out.paid, 1);
    assert!(out.batch_paid);
    assert_eq!(m.accepted(), 1);
    assert_books(&pool).await;
}
