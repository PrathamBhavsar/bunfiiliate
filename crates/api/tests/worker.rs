use api::worker::{approve_due, process_events};
use bf_db::ledger::balance_for_affiliate;
use bf_db::tracker::{InboundEvent, insert_events};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// A program with two active affiliates (A, B), each with a referral code.
struct Seed {
    program: Uuid,
    a: Uuid,
    b: Uuid,
}

/// (kind, rate_bps, amount_cents, duration, months)
type PlanSpec = (
    &'static str,
    Option<i32>,
    Option<i64>,
    &'static str,
    Option<i32>,
);

struct Opts {
    model: &'static str,
    auto_approve: bool,
    hold_days: i32,
    plan: PlanSpec,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            model: "last_touch",
            auto_approve: true,
            hold_days: 0,
            plan: ("percent", Some(1000), None, "lifetime", None),
        }
    }
}

async fn new_plan(
    pool: &PgPool,
    program: Uuid,
    (kind, rate, amount, duration, months): PlanSpec,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO commission_plans (id, program_id, kind, rate_bps, amount_cents, duration, months) \
         VALUES ($1, $2, $3::commission_kind, $4, $5, $6::commission_duration, $7)",
    )
    .bind(id)
    .bind(program)
    .bind(kind)
    .bind(rate)
    .bind(amount)
    .bind(duration)
    .bind(months)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn affiliate(pool: &PgPool, workspace: Uuid, program: Uuid, code: &str) -> Uuid {
    let (user, aff) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO users (id, workspace_id, email, password_hash, role) \
         VALUES ($1, $2, $3, 'h', 'affiliate')",
    )
    .bind(user)
    .bind(workspace)
    .bind(format!("{user}@x.io"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO affiliates (id, workspace_id, user_id, status) VALUES ($1, $2, $3, 'active')",
    )
    .bind(aff)
    .bind(workspace)
    .bind(user)
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

async fn seed(pool: &PgPool, o: Opts) -> Seed {
    let (workspace, program) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', $2)")
        .bind(workspace)
        .bind(workspace.to_string())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, attribution_model, auto_approve, \
           hold_days, public_key, webhook_secret) \
         VALUES ($1, $2, 'p', 'USD', $3::attribution_model, $4, $5, $6, 's')",
    )
    .bind(program)
    .bind(workspace)
    .bind(o.model)
    .bind(o.auto_approve)
    .bind(o.hold_days)
    .bind(program.to_string())
    .execute(pool)
    .await
    .unwrap();
    let plan = new_plan(pool, program, o.plan).await;
    sqlx::query("UPDATE programs SET default_plan_id = $2 WHERE id = $1")
        .bind(program)
        .bind(plan)
        .execute(pool)
        .await
        .unwrap();
    let a = affiliate(pool, workspace, program, &format!("A{program}")).await;
    let b = affiliate(pool, workspace, program, &format!("B{program}")).await;
    Seed { program, a, b }
}

async fn click(pool: &PgPool, s: &Seed, affiliate: Uuid, visitor: &str, ts: DateTime<Utc>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO clicks (id, program_id, affiliate_id, code, visitor_id, ts) \
         VALUES ($1, $2, $3, 'c', $4, $5)",
    )
    .bind(id)
    .bind(s.program)
    .bind(affiliate)
    .bind(visitor)
    .bind(ts)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn send(pool: &PgPool, s: &Seed, source: &str, external_id: &str, payload: Value) -> u64 {
    insert_events(
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
    .unwrap()
}

fn sale(customer: &str, cents: i64, click: Option<Uuid>, at: DateTime<Utc>) -> Value {
    json!({
        "customer": customer,
        "amount_cents": cents,
        "click": click,
        "occurred_at": at.to_rfc3339(),
    })
}

/// `(affiliate_id, commission_cents, status)` of every conversion, oldest first.
async fn conversions(pool: &PgPool, s: &Seed) -> Vec<(Uuid, i64, String)> {
    sqlx::query_as(
        "SELECT affiliate_id, commission_cents, status::text FROM conversions \
         WHERE program_id = $1 ORDER BY occurred_at, id",
    )
    .bind(s.program)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn unbalanced_txns(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT txn_id FROM ledger_entries GROUP BY txn_id \
         HAVING sum(amount_cents) <> 0) t",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn outbox_topics(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT topic FROM outbox ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn flow_click_to_approved(pool: PgPool) {
    let s = seed(&pool, Opts::default()).await;
    let now = Utc::now();
    let c = click(&pool, &s, s.a, "v1", now - Duration::hours(2)).await;

    assert_eq!(
        send(
            &pool,
            &s,
            "generic",
            "order-1",
            sale("cus_1", 10_000, Some(c), now)
        )
        .await,
        1
    );
    assert_eq!(process_events(&pool).await.unwrap(), 1);

    assert_eq!(
        conversions(&pool, &s).await,
        [(s.a, 1_000, "pending".into())]
    );
    let bal = balance_for_affiliate(&pool, s.program, s.a).await.unwrap();
    assert_eq!((bal.pending_cents, bal.payable_cents), (1_000, 0));
    let (done, err): (bool, Option<String>) =
        sqlx::query_as("SELECT processed_at IS NOT NULL, error FROM inbound_events")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(done && err.is_none());
    assert_eq!(outbox_topics(&pool).await, ["conversion.created"]);
    let first_click: Option<Uuid> = sqlx::query_scalar("SELECT first_click_id FROM customers")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(first_click, Some(c));

    assert_eq!(approve_due(&pool).await.unwrap(), 1);
    assert_eq!(
        conversions(&pool, &s).await,
        [(s.a, 1_000, "approved".into())]
    );
    let bal = balance_for_affiliate(&pool, s.program, s.a).await.unwrap();
    assert_eq!((bal.pending_cents, bal.payable_cents), (0, 1_000));
    assert_eq!(
        outbox_topics(&pool).await,
        ["conversion.created", "conversion.approved"]
    );
    assert_eq!(unbalanced_txns(&pool).await, 0);
    let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(entries, 4); // create (2 legs) + approve (2 legs)

    // Nothing left to do.
    assert_eq!(approve_due(&pool).await.unwrap(), 0);
    assert_eq!(process_events(&pool).await.unwrap(), 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn approve_waits_for_hold_and_flag(pool: PgPool) {
    let held = seed(
        &pool,
        Opts {
            hold_days: 7,
            ..Opts::default()
        },
    )
    .await;
    let manual = seed(
        &pool,
        Opts {
            auto_approve: false,
            ..Opts::default()
        },
    )
    .await;
    let now = Utc::now();
    for (s, ext) in [(&held, "h1"), (&manual, "m1")] {
        let c = click(&pool, s, s.a, "v", now - Duration::days(20)).await;
        send(
            &pool,
            s,
            "generic",
            ext,
            sale("cus", 1_000, Some(c), now - Duration::days(10)),
        )
        .await;
    }
    send(
        &pool,
        &held,
        "generic",
        "h2",
        sale("cus2", 1_000, None, now),
    )
    .await; // organic
    assert_eq!(process_events(&pool).await.unwrap(), 3);

    // `held`: 10 days old, hold is 7 -> approves. `manual`: never auto-approves.
    assert_eq!(approve_due(&pool).await.unwrap(), 1);
    assert_eq!(conversions(&pool, &held).await[0].2, "approved");
    assert_eq!(conversions(&pool, &manual).await[0].2, "pending");

    // Move it inside the hold window: stays pending.
    let fresh = seed(
        &pool,
        Opts {
            hold_days: 7,
            ..Opts::default()
        },
    )
    .await;
    let c = click(&pool, &fresh, fresh.a, "v", now - Duration::days(3)).await;
    send(
        &pool,
        &fresh,
        "generic",
        "f1",
        sale("cus", 1_000, Some(c), now - Duration::days(2)),
    )
    .await;
    process_events(&pool).await.unwrap();
    assert_eq!(approve_due(&pool).await.unwrap(), 0);
    assert_eq!(conversions(&pool, &fresh).await[0].2, "pending");
    assert_eq!(unbalanced_txns(&pool).await, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn duplicate_event_is_one_conversion(pool: PgPool) {
    let s = seed(&pool, Opts::default()).await;
    let now = Utc::now();
    let c = click(&pool, &s, s.a, "v1", now - Duration::hours(1)).await;
    let payload = sale("cus_1", 10_000, Some(c), now);

    assert_eq!(
        send(&pool, &s, "generic", "order-1", payload.clone()).await,
        1
    );
    assert_eq!(
        send(&pool, &s, "generic", "order-1", payload.clone()).await,
        0
    ); // redelivery
    assert_eq!(process_events(&pool).await.unwrap(), 1);

    // Replay: the event is processed again but must not convert twice.
    sqlx::query("UPDATE inbound_events SET processed_at = NULL")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(process_events(&pool).await.unwrap(), 1);

    assert_eq!(conversions(&pool, &s).await.len(), 1);
    let txns: i64 = sqlx::query_scalar("SELECT count(DISTINCT txn_id) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(txns, 1);
    assert_eq!(outbox_topics(&pool).await.len(), 1);
    let bal = balance_for_affiliate(&pool, s.program, s.a).await.unwrap();
    assert_eq!(bal.pending_cents, 1_000);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn recurring_follows_customer_and_duration_window(pool: PgPool) {
    let windows = [
        (
            ("percent", Some(1000), None, "one_month", None),
            [1_000, 0, 0],
        ),
        (
            ("percent", Some(1000), None, "x_months", Some(2)),
            [1_000, 1_000, 0],
        ),
        (
            ("percent", Some(1000), None, "lifetime", None),
            [1_000, 1_000, 1_000],
        ),
        (
            ("flat", None, Some(250), "x_months", Some(2)),
            [250, 250, 0],
        ),
    ];
    for (plan, want) in windows {
        let s = seed(
            &pool,
            Opts {
                plan,
                ..Opts::default()
            },
        )
        .await;
        let t0: DateTime<Utc> = "2026-01-10T12:00:00Z".parse().unwrap();
        let c = click(&pool, &s, s.a, "v1", t0 - Duration::days(1)).await;
        // Month 0 carries the click; later months carry none (and a stray code of B):
        // the customer, not the event, decides the affiliate.
        let months = [
            ("o1", sale("cus_1", 10_000, Some(c), t0)),
            (
                "o2",
                sale(
                    "cus_1",
                    10_000,
                    None,
                    "2026-02-12T12:00:00Z".parse().unwrap(),
                ),
            ),
            (
                "o3",
                sale(
                    "cus_1",
                    10_000,
                    None,
                    "2026-03-10T12:00:00Z".parse().unwrap(),
                ),
            ),
        ];
        for (ext, mut payload) in months {
            payload["code"] = json!(format!("B{}", s.program));
            send(&pool, &s, "generic", ext, payload).await;
            process_events(&pool).await.unwrap();
        }
        let got = conversions(&pool, &s).await;
        assert_eq!(
            got.iter().map(|c| c.1).collect::<Vec<_>>(),
            want,
            "{plan:?}"
        );
        assert!(got.iter().all(|c| c.0 == s.a), "{plan:?}");
        let bal = balance_for_affiliate(&pool, s.program, s.a).await.unwrap();
        assert_eq!(bal.pending_cents, want.iter().sum::<i64>());
        assert_eq!(
            balance_for_affiliate(&pool, s.program, s.b)
                .await
                .unwrap()
                .pending_cents,
            0
        );
    }
    assert_eq!(unbalanced_txns(&pool).await, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn first_vs_last_touch(pool: PgPool) {
    let now = Utc::now();
    for (model, winner_is_a) in [("first_touch", true), ("last_touch", false)] {
        let s = seed(
            &pool,
            Opts {
                model,
                ..Opts::default()
            },
        )
        .await;
        // One visitor clicks A, then B; the cookie (and so the event) carries B's click.
        click(&pool, &s, s.a, "v1", now - Duration::days(3)).await;
        let last = click(&pool, &s, s.b, "v1", now - Duration::days(1)).await;
        // Another visitor's click and an expired one must not count.
        click(&pool, &s, s.b, "v2", now - Duration::days(4)).await;
        click(&pool, &s, s.b, "v1", now - Duration::days(90)).await;
        send(
            &pool,
            &s,
            "generic",
            "o1",
            sale("cus_1", 10_000, Some(last), now),
        )
        .await;
        process_events(&pool).await.unwrap();
        let got = conversions(&pool, &s).await;
        assert_eq!(got.len(), 1, "{model}");
        assert_eq!(got[0].0, if winner_is_a { s.a } else { s.b }, "{model}");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn referral_code_without_click_and_plan_override(pool: PgPool) {
    let s = seed(&pool, Opts::default()).await;
    let flat = new_plan(
        &pool,
        s.program,
        ("flat", None, Some(700), "lifetime", None),
    )
    .await;
    sqlx::query("UPDATE enrollments SET plan_override_id = $2 WHERE affiliate_id = $1")
        .bind(s.b)
        .bind(flat)
        .execute(&pool)
        .await
        .unwrap();
    let now = Utc::now();
    // Stripe delivery crediting B by `bf_ref` metadata: override beats the 10% default.
    send(
        &pool,
        &s,
        "stripe",
        "evt_1",
        json!({"id": "evt_1", "type": "invoice.paid", "created": now.timestamp(),
               "data": {"object": {"customer": "cus_s", "amount_paid": 20_000, "currency": "usd",
                        "metadata": {"bf_ref": format!("B{}", s.program)}}}}),
    )
    .await;
    // Not a conversion: processed, nothing recorded.
    send(
        &pool,
        &s,
        "stripe",
        "evt_2",
        json!({"id": "evt_2", "type": "customer.created"}),
    )
    .await;
    // Same sale for A: default plan.
    send(
        &pool,
        &s,
        "generic",
        "o2",
        json!({"customer": "cus_g", "amount_cents": 20_000, "code": format!("A{}", s.program),
               "occurred_at": now.to_rfc3339()}),
    )
    .await;
    assert_eq!(process_events(&pool).await.unwrap(), 3);
    let mut got = conversions(&pool, &s).await;
    got.sort_by_key(|c| c.1);
    assert_eq!(
        got.iter().map(|c| (c.0, c.1)).collect::<Vec<_>>(),
        [(s.b, 700), (s.a, 2_000)]
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn bad_events_are_marked_not_retried(pool: PgPool) {
    let s = seed(&pool, Opts::default()).await;
    let now = Utc::now();
    let c = click(&pool, &s, s.a, "v1", now - Duration::hours(1)).await;
    send(
        &pool,
        &s,
        "generic",
        "no-customer",
        json!({"amount_cents": 5}),
    )
    .await;
    send(
        &pool,
        &s,
        "generic",
        "bad-currency",
        json!({"customer": "c", "amount_cents": 5, "currency": "EUR", "click": c}),
    )
    .await;
    send(
        &pool,
        &s,
        "generic",
        "bad-type",
        json!({"customer": "c", "amount_cents": 5, "type": "nope", "click": c}),
    )
    .await;
    send(
        &pool,
        &s,
        "generic",
        "negative",
        sale("c", -5, Some(c), now),
    )
    .await;
    send(&pool, &s, "generic", "organic", sale("other", 5, None, now)).await;
    assert_eq!(process_events(&pool).await.unwrap(), 5);
    assert_eq!(process_events(&pool).await.unwrap(), 0);

    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT external_id, error FROM inbound_events ORDER BY external_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let err = |ext: &str| rows.iter().find(|r| r.0 == ext).unwrap().1.clone();
    assert_eq!(err("no-customer").as_deref(), Some("missing customer"));
    assert!(err("bad-currency").unwrap().contains("currency"));
    assert!(err("bad-type").unwrap().contains("conversion type"));
    assert_eq!(err("negative").as_deref(), Some("negative amount"));
    assert_eq!(err("organic"), None);
    assert!(conversions(&pool, &s).await.is_empty());
    // Rejected events leave no partial writes (the customer upsert was rolled back).
    let customers: i64 = sqlx::query_scalar("SELECT count(*) FROM customers")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(customers, 0);
}
