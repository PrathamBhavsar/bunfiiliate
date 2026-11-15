use api::error::AppError;
use api::payout_requests::{approve_request, create_request, reject_request};
use api::payouts::{approve_batch, mark_paid};
use api::worker::{approve_due, process_events};
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
    approve_due(pool).await.unwrap();
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

async fn ws_of(pool: &PgPool, s: &Seed) -> Uuid {
    sqlx::query_scalar("SELECT workspace_id FROM programs WHERE id = $1")
        .bind(s.program)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn req(pool: &PgPool, s: &Seed, aff: Uuid, cents: i64) -> Result<Uuid, AppError> {
    create_request(pool, ws_of(pool, s).await, aff, s.program, cents, "USD").await
}

fn code(e: AppError) -> u16 {
    use axum::response::IntoResponse;
    e.into_response().status().as_u16()
}

async fn notes(pool: &PgPool, aff: Uuid, kind: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM notifications n JOIN affiliates a ON a.user_id = n.user_id \
         WHERE a.id = $1 AND n.kind = $2",
    )
    .bind(aff)
    .bind(kind)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn request_capped_at_payable_balance(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await; // 1000 payable
    assert_eq!(code(req(&pool, &s, s.a, 1001).await.unwrap_err()), 400);
    assert_eq!(code(req(&pool, &s, s.a, 0).await.unwrap_err()), 400);
    req(&pool, &s, s.a, 1000).await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn request_blocked_below_minimum(pool: PgPool) {
    let s = seed(&pool, true).await;
    sqlx::query("UPDATE affiliates SET payout_min_cents = 500 WHERE id = $1")
        .bind(s.a)
        .execute(&pool)
        .await
        .unwrap();
    sale(&pool, &s, "o1", 10_000, s.a).await;
    assert_eq!(code(req(&pool, &s, s.a, 499).await.unwrap_err()), 400);
    req(&pool, &s, s.a, 500).await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn request_currency_must_match_program(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    let ws = ws_of(&pool, &s).await;
    let e = create_request(&pool, ws, s.a, s.program, 100, "EUR").await;
    assert_eq!(code(e.unwrap_err()), 400);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn approve_rejects_when_program_currency_changed(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    let id = req(&pool, &s, s.a, 1000).await.unwrap();
    sqlx::query("UPDATE programs SET currency = 'EUR' WHERE id = $1")
        .bind(s.program)
        .execute(&pool)
        .await
        .unwrap();
    let ws = ws_of(&pool, &s).await;
    let e = approve_request(&pool, ws, id, s.admin).await.unwrap_err();
    assert_eq!(code(e), 409);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn duplicate_pending_request_conflicts(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    req(&pool, &s, s.a, 500).await.unwrap();
    assert_eq!(code(req(&pool, &s, s.a, 500).await.unwrap_err()), 409);
    // Another affiliate is unaffected.
    sale(&pool, &s, "o2", 10_000, s.b).await;
    req(&pool, &s, s.b, 500).await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_requests_only_one_wins(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    let ws = ws_of(&pool, &s).await;
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let (pool, program, aff) = (pool.clone(), s.program, s.a);
            tokio::spawn(async move { create_request(&pool, ws, aff, program, 500, "USD").await })
        })
        .collect();
    let mut ok = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(_) => ok += 1,
            Err(e) => assert_eq!(code(e), 409),
        }
    }
    assert_eq!(ok, 1);
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payout_requests WHERE status = 'pending'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn approve_sizes_from_current_balance_not_requested_amount(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await; // 1000
    sale(&pool, &s, "o2", 10_000, s.a).await; // 2000 total
    let id = req(&pool, &s, s.a, 2000).await.unwrap();
    reverse(&pool, &s, "refund", "o2").await; // payable drops to 1000
    let ws = ws_of(&pool, &s).await;
    let amount = approve_request(&pool, ws, id, s.admin).await.unwrap();
    assert_eq!(amount, 1000);
    let (status, payout, paid): (String, Uuid, i64) = sqlx::query_as(
        "SELECT r.status::text, r.payout_id, p.amount_cents FROM payout_requests r \
         JOIN payouts p ON p.id = r.payout_id WHERE r.id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((status.as_str(), paid), ("approved", 1000));
    assert_eq!(notes(&pool, s.a, "payout_request.approved").await, 1);
    // Deciding twice is a conflict; the payout follows the normal batch flow and closes the request.
    assert_eq!(
        code(approve_request(&pool, ws, id, s.admin).await.unwrap_err()),
        409
    );
    let batch: Uuid = sqlx::query_scalar("SELECT batch_id FROM payouts WHERE id = $1")
        .bind(payout)
        .fetch_one(&pool)
        .await
        .unwrap();
    approve_batch(&pool, ws, batch, s.admin).await.unwrap();
    mark_paid(&pool, ws, batch).await.unwrap();
    let st: String = sqlx::query_scalar("SELECT status::text FROM payout_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(st, "paid");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn approve_with_nothing_payable_conflicts_and_stays_pending(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    let id = req(&pool, &s, s.a, 1000).await.unwrap();
    reverse(&pool, &s, "refund", "o1").await;
    let ws = ws_of(&pool, &s).await;
    assert_eq!(
        code(approve_request(&pool, ws, id, s.admin).await.unwrap_err()),
        409
    );
    let st: String = sqlx::query_scalar("SELECT status::text FROM payout_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(st, "pending");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_approvals_create_one_payout(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    let id = req(&pool, &s, s.a, 1000).await.unwrap();
    let ws = ws_of(&pool, &s).await;
    let tasks: Vec<_> = (0..4)
        .map(|_| {
            let (pool, admin) = (pool.clone(), s.admin);
            tokio::spawn(async move { approve_request(&pool, ws, id, admin).await })
        })
        .collect();
    let mut ok = 0;
    for t in tasks {
        if t.await.unwrap().is_ok() {
            ok += 1;
        }
    }
    assert_eq!(ok, 1);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM payouts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reject_requires_reason_and_notifies(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    let id = req(&pool, &s, s.a, 500).await.unwrap();
    let ws = ws_of(&pool, &s).await;
    for bad in ["", "   "] {
        let e = reject_request(&pool, ws, id, s.admin, bad)
            .await
            .unwrap_err();
        assert_eq!(code(e), 400);
    }
    reject_request(&pool, ws, id, s.admin, "bad data")
        .await
        .unwrap();
    assert_eq!(notes(&pool, s.a, "payout_request.rejected").await, 1);
    // A rejected request frees the slot for a new one.
    req(&pool, &s, s.a, 500).await.unwrap();
    assert_eq!(
        code(
            reject_request(&pool, ws, id, s.admin, "x")
                .await
                .unwrap_err()
        ),
        409
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn other_workspace_cannot_decide(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    let id = req(&pool, &s, s.a, 500).await.unwrap();
    let other = Uuid::now_v7();
    assert_eq!(
        code(
            approve_request(&pool, other, id, s.admin)
                .await
                .unwrap_err()
        ),
        404
    );
    assert_eq!(
        code(
            reject_request(&pool, other, id, s.admin, "x")
                .await
                .unwrap_err()
        ),
        404
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn creating_a_request_notifies_admins(pool: PgPool) {
    let s = seed(&pool, true).await;
    sale(&pool, &s, "o1", 10_000, s.a).await;
    req(&pool, &s, s.a, 500).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'payout_request.created'",
    )
    .bind(s.admin)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}
