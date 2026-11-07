use bf_db::billing::{plan_for_workspace, usage_at, usage_for_workspace};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

async fn raw_ws(pool: &PgPool, slug: &str) -> Uuid {
    let ws = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, $2, $2)")
        .bind(ws)
        .bind(slug)
        .execute(pool)
        .await
        .unwrap();
    ws
}

#[sqlx::test(migrations = "./migrations")]
async fn free_plan_seeded_unlimited_default(pool: PgPool) {
    let ws = raw_ws(&pool, "a").await;
    let p = plan_for_workspace(&pool, ws).await.unwrap();
    assert_eq!(p.key, "free");
    assert!(p.is_default);
    assert_eq!(p.price_cents, 0);
    assert!(
        p.max_programs.is_none() && p.max_affiliates.is_none() && p.max_monthly_clicks.is_none()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn only_one_default_plan_allowed(pool: PgPool) {
    let err = sqlx::query(
        "INSERT INTO billing_plans (id, key, name, price_cents, is_default) \
         VALUES ($1, 'pro', 'Pro', 900, true)",
    )
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(err.to_string().contains("billing_plans_one_default"));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_workspace_inserts_default_subscription(pool: PgPool) {
    let ws = Uuid::now_v7();
    bf_db::auth::create_workspace(&pool, ws, "w", "w")
        .await
        .unwrap();
    let (n, default): (i64, bool) = sqlx::query_as(
        "SELECT count(*), bool_and(p.is_default) FROM subscriptions s \
         JOIN billing_plans p ON p.id = s.plan_id WHERE s.workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((n, default), (1, true));
}

#[sqlx::test(migrations = "./migrations")]
async fn every_workspace_has_a_default_subscription_after_backfill(pool: PgPool) {
    let ws = raw_ws(&pool, "old").await; // old path: no subscription row
    let backfill = include_str!("../migrations/0077_billing.sql")
        .split("-- Backfill")
        .nth(1)
        .unwrap();
    let sql = backfill.split_once('\n').unwrap().1;
    sqlx::query(sql).execute(&pool).await.unwrap();
    sqlx::query(sql).execute(&pool).await.unwrap(); // idempotent
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM subscriptions s JOIN billing_plans p ON p.id = s.plan_id \
         WHERE s.workspace_id = $1 AND p.is_default",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_for_workspace_counts_match_live_tables(pool: PgPool) {
    let ws = raw_ws(&pool, "u").await;
    let other = raw_ws(&pool, "o").await;
    let mut first_prog = None;
    let mut first_aff = None;
    for (w, np, na) in [(ws, 3, 5), (other, 1, 2)] {
        for i in 0..np {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
                 VALUES ($1, $2, 'p', 'USD', $3, 's')",
            )
            .bind(id)
            .bind(w)
            .bind(id.to_string())
            .execute(&pool)
            .await
            .unwrap();
            if w == ws && i == 0 {
                first_prog = Some(id);
            }
        }
        for _ in 0..na {
            let (u, a) = (Uuid::now_v7(), Uuid::now_v7());
            sqlx::query(
                "INSERT INTO users (id, workspace_id, email, password_hash, role) \
                 VALUES ($1, $2, $3, 'h', 'affiliate')",
            )
            .bind(u)
            .bind(w)
            .bind(format!("{u}@x.io"))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO affiliates (id, workspace_id, user_id) VALUES ($1, $2, $3)")
                .bind(a)
                .bind(w)
                .bind(u)
                .execute(&pool)
                .await
                .unwrap();
            if w == ws && first_aff.is_none() {
                first_aff = Some(a);
            }
        }
    }
    let now = Utc::now();
    for age_days in [1, 29, 31] {
        sqlx::query(
            "INSERT INTO clicks (id, program_id, affiliate_id, code, visitor_id, ts) \
             VALUES ($1, $2, $3, 'c', 'v', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(first_prog.unwrap())
        .bind(first_aff.unwrap())
        .bind(now - Duration::days(age_days))
        .execute(&pool)
        .await
        .unwrap();
    }
    let u = usage_at(&pool, ws, now).await.unwrap();
    assert_eq!((u.programs, u.affiliates, u.monthly_clicks), (3, 5, 2));
    assert_eq!(usage_for_workspace(&pool, ws).await.unwrap().programs, 3);
}
