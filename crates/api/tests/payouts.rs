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
    let batch = create_batch(pool, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    approve_batch(pool, batch, s.admin).await.unwrap();
    mark_paid(pool, batch).await.unwrap();
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
    assert_eq!(create_batch(&pool, s.program, s.admin).await.unwrap(), None); // none approved yet
    approve_due(&pool).await.unwrap();

    let batch = create_batch(&pool, s.program, s.admin)
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
    assert_eq!(create_batch(&pool, s.program, s.admin).await.unwrap(), None);

    // B reaches its minimum with a second sale; A earns a little more.
    sale(&pool, &s, "o3", 40_000, s.b).await; // +4000 -> 5000
    sale(&pool, &s, "o4", 1_000, s.a).await; // +100
    approve_due(&pool).await.unwrap();
    let second = create_batch(&pool, s.program, s.admin)
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

    let batch = create_batch(&pool, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    // A draft batch cannot be paid.
    assert!(matches!(
        mark_paid(&pool, batch).await,
        Err(AppError::Conflict(_))
    ));
    approve_batch(&pool, batch, s.admin).await.unwrap();
    assert!(matches!(
        approve_batch(&pool, batch, s.admin).await,
        Err(AppError::Conflict(_))
    ));
    assert!(matches!(
        cancel_batch(&pool, batch).await,
        Err(AppError::Conflict(_))
    ));
    assert_books(&pool).await;

    mark_paid(&pool, batch).await.unwrap();
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
        mark_paid(&pool, batch).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(ledger_ids(&pool).await, before);
    assert!(matches!(
        mark_paid(&pool, Uuid::now_v7()).await,
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
    assert_eq!(create_batch(&pool, s.program, s.admin).await.unwrap(), None);

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
    let batch = create_batch(&pool, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    approve_batch(&pool, batch, s.admin).await.unwrap();

    reverse(&pool, &s, "refund", "o2").await; // 3000 of the 4000
    mark_paid(&pool, batch).await.unwrap();
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
    let first = create_batch(&pool, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    cancel_batch(&pool, first).await.unwrap();
    assert!(matches!(
        approve_batch(&pool, first, s.admin).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(balance(&pool, &s, s.a).await, (0, 1_000));

    let second = create_batch(&pool, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first, second);
    approve_batch(&pool, second, s.admin).await.unwrap();
    mark_paid(&pool, second).await.unwrap();
    assert_eq!(status_of(&pool, "o1").await, "paid");
    assert_books(&pool).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payouts_csv_export_is_exact(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 12_345, s.a).await; // 10% of 12345 = 1234.5, rounds half up to 1235
    sale(&pool, &s, "o2", 5_000, s.b).await;
    approve_due(&pool).await.unwrap();
    let batch = create_batch(&pool, s.program, s.admin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        batch_csv(&pool, batch).await.unwrap(),
        "email,amount,currency\na@x.io,12.35,USD\nb@x.io,5.00,USD\n"
    );
    assert!(matches!(
        batch_csv(&pool, Uuid::now_v7()).await,
        Err(AppError::NotFound(_))
    ));
}
