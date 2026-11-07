//! C20-C22: partition pruning on the clicks queries, and rollups vs retention.

use api::analytics::{ROLLUP_CLICK_LEG, rollup};
use bf_db::partitions::{drop_expired_partitions, ensure_partitions};
use chrono::{Datelike, Duration, Months, NaiveDate, TimeZone, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

async fn seed(pool: &PgPool) -> (Uuid, Uuid) {
    let (ws, prog, user, aff) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    bf_db::auth::create_workspace(pool, ws, "w", "w")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
         VALUES ($1, $2, 'p', 'USD', $1::text, 's')",
    )
    .bind(prog)
    .bind(ws)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users (id, workspace_id, email, password_hash, role) \
         VALUES ($1, $2, 'a@x.io', 'h', 'affiliate')",
    )
    .bind(user)
    .bind(ws)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO affiliates (id, workspace_id, user_id) VALUES ($1, $2, $3)")
        .bind(aff)
        .bind(ws)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    (prog, aff)
}

/// Names of every relation the plan scans.
fn relations(plan: &Value, out: &mut Vec<String>) {
    match plan {
        Value::Object(m) => {
            if let Some(Value::String(r)) = m.get("Relation Name") {
                out.push(r.clone());
            }
            m.values().for_each(|v| relations(v, out));
        }
        Value::Array(a) => a.iter().for_each(|v| relations(v, out)),
        _ => {}
    }
}

fn month_of(offset: u32) -> NaiveDate {
    let n = Utc::now();
    NaiveDate::from_ymd_opt(n.year(), n.month(), 1).unwrap() - Months::new(offset)
}

fn pruned_to(plan: &[Value], what: &str) -> Vec<String> {
    let mut rels = Vec::new();
    relations(&Value::Array(plan.to_vec()), &mut rels);
    rels.retain(|r| r.starts_with("clicks_y") || r == "clicks_default");
    assert!(
        !rels.is_empty(),
        "{what}: plan scans no clicks partition at all"
    );
    rels
}

/// C21: the rollup's click leg, given a 7-day window, never scans months before that window.
#[sqlx::test(migrations = "../db/migrations")]
async fn clicks_queries_prune_partitions(pool: PgPool) {
    let (prog, _) = seed(&pool).await;
    for back in 0..12 {
        let d = month_of(back);
        ensure_partitions(
            &pool,
            0,
            Utc.from_utc_datetime(&d.and_hms_opt(1, 0, 0).unwrap()),
        )
        .await
        .unwrap();
    }
    let from = (Utc::now() - Duration::days(7)).date_naive();
    let plan: Vec<Value> = sqlx::query_scalar(&format!(
        "EXPLAIN (FORMAT JSON) INSERT INTO daily_rollups \
         (program_id, affiliate_id, day, dim, clicks) \
         SELECT $1, affiliate_id, day, 'total', sum(clicks) FROM ({ROLLUP_CLICK_LEG}) f \
         GROUP BY affiliate_id, day"
    ))
    .bind(prog)
    .bind(from)
    .fetch_all(&pool)
    .await
    .unwrap();
    let rels = pruned_to(&plan, "rollup click leg");
    // The leg has a lower bound only: every month before the window is pruned.
    let floor = format!("clicks_y{:04}m{:02}", from.year(), from.month());
    let stale: Vec<_> = rels
        .iter()
        .filter(|r| r.starts_with("clicks_y") && **r < floor)
        .collect();
    assert!(
        stale.is_empty(),
        "scanned months before the window: {stale:?}"
    );
    // Unbounded (None) must still work and scan everything.
    let none: Option<NaiveDate> = None;
    let plan: Vec<Value> = sqlx::query_scalar(&format!("EXPLAIN (FORMAT JSON) {ROLLUP_CLICK_LEG}"))
        .bind(prog)
        .bind(none)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(pruned_to(&plan, "unbounded").len() > 10);
}

/// C22: a rollup rebuild never reaches below the retention floor, so dropped days keep their rows.
#[sqlx::test(migrations = "../db/migrations")]
async fn rollup_never_rebuilds_days_before_retention_floor(pool: PgPool) {
    let (prog, aff) = seed(&pool).await;
    let old = Utc::now().date_naive() - Duration::days(400);
    sqlx::query(
        "INSERT INTO daily_rollups (program_id, affiliate_id, day, dim, clicks) \
         VALUES ($1, $2, $3, 'total', 5)",
    )
    .bind(prog)
    .bind(aff)
    .bind(old)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO clicks_retention (floor_day) VALUES ($1)")
        .bind(old + Duration::days(10))
        .execute(&pool)
        .await
        .unwrap();
    rollup(&pool).await.unwrap();
    let kept: i64 = sqlx::query_scalar("SELECT clicks FROM daily_rollups WHERE day = $1")
        .bind(old)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, 5, "rollup for a dropped day must survive");
}

/// Control for the test above: without a floor the same rebuild does recompute the day.
#[sqlx::test(migrations = "../db/migrations")]
async fn rollup_rebuilds_old_days_when_no_floor(pool: PgPool) {
    let (prog, aff) = seed(&pool).await;
    let old = Utc::now().date_naive() - Duration::days(400);
    sqlx::query(
        "INSERT INTO daily_rollups (program_id, affiliate_id, day, dim, clicks) \
         VALUES ($1, $2, $3, 'total', 5)",
    )
    .bind(prog)
    .bind(aff)
    .bind(old)
    .execute(&pool)
    .await
    .unwrap();
    rollup(&pool).await.unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM daily_rollups WHERE day = $1")
        .bind(old)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        rows, 0,
        "no clicks exist for that day, so the rebuild empties it"
    );
}

/// C22 end to end: drop a rolled-up partition, then a rollup tick keeps that day's row.
#[sqlx::test(migrations = "../db/migrations")]
async fn retention_then_rollup_keeps_dropped_days(pool: PgPool) {
    let (prog, aff) = seed(&pool).await;
    let old_month = month_of(8);
    let ts = Utc.from_utc_datetime(&old_month.and_hms_opt(12, 0, 0).unwrap());
    ensure_partitions(&pool, 0, ts).await.unwrap();
    sqlx::query(
        "INSERT INTO clicks (id, program_id, affiliate_id, code, visitor_id, ts) \
         VALUES ($1, $2, $3, 'c', 'v', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(prog)
    .bind(aff)
    .bind(ts)
    .execute(&pool)
    .await
    .unwrap();
    rollup(&pool).await.unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT sum(clicks)::bigint FROM daily_rollups WHERE dim = 'total'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, 1);
    let r = drop_expired_partitions(&pool, 3, Utc::now()).await.unwrap();
    assert_eq!(r.dropped.len(), 1, "{r:?}");
    rollup(&pool).await.unwrap();
    let after: i64 =
        sqlx::query_scalar("SELECT sum(clicks)::bigint FROM daily_rollups WHERE dim = 'total'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, 1, "the dropped day's rollup survives later ticks");
}

/// C30: the per-code stats click query is bounded on `ts`, so months outside the range
/// are pruned on both sides.
#[sqlx::test(migrations = "../db/migrations")]
async fn per_code_stats_query_has_ts_bound(pool: PgPool) {
    let (prog, _) = seed(&pool).await;
    for back in 0..12 {
        let d = month_of(back);
        ensure_partitions(
            &pool,
            0,
            Utc.from_utc_datetime(&d.and_hms_opt(1, 0, 0).unwrap()),
        )
        .await
        .unwrap();
    }
    let to = Utc::now();
    let from = to - Duration::days(7);
    let plan: Vec<Value> = sqlx::query_scalar(&format!(
        "EXPLAIN (FORMAT JSON) {}",
        bf_db::code_stats::CLICK_SQL
    ))
    .bind(prog)
    .bind("abc")
    .bind(from)
    .bind(to)
    .fetch_all(&pool)
    .await
    .unwrap();
    let rels = pruned_to(&plan, "code stats click query");
    let floor = format!("clicks_y{:04}m{:02}", from.year(), from.month());
    let stale: Vec<_> = rels
        .iter()
        .filter(|r| r.starts_with("clicks_y") && **r < floor)
        .collect();
    assert!(
        stale.is_empty(),
        "months before the range scanned: {stale:?}"
    );
    let ceil = format!("clicks_y{:04}m{:02}", to.year(), to.month());
    let future: Vec<_> = rels
        .iter()
        .filter(|r| r.starts_with("clicks_y") && **r > ceil)
        .collect();
    assert!(
        future.is_empty(),
        "months after the range scanned: {future:?}"
    );
}
