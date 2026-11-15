use bf_db::partitions::{
    default_rows, drop_expired_partitions, ensure_partitions, month_partitions, retention_floor,
};
use bf_db::tracker::{Click, insert_clicks};
use chrono::{DateTime, Datelike, Months, NaiveDate, TimeZone, Utc};
use sqlx::PgPool;
use uuid::Uuid;

fn at(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap()
}

/// 12:00 UTC on the 15th, `n` months before the current month.
fn ago(n: u32) -> DateTime<Utc> {
    let now = Utc::now();
    let d = NaiveDate::from_ymd_opt(now.year(), now.month(), 15).unwrap() - Months::new(n);
    Utc.from_utc_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
}

/// One workspace, program and affiliate; returns `(program, affiliate)`.
async fn seed(pool: &PgPool) -> (Uuid, Uuid) {
    let (ws, prog, user, aff) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', $1::text)")
        .bind(ws)
        .execute(pool)
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

async fn click(pool: &PgPool, p: (Uuid, Uuid), id: Uuid, ts: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO clicks (id, program_id, affiliate_id, code, visitor_id, ts) \
         VALUES ($1, $2, $3, 'c', 'v', $4) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(p.0)
    .bind(p.1)
    .bind(ts)
    .execute(pool)
    .await
    .unwrap();
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn ensure_partitions_idempotent(pool: PgPool) {
    let now = at(2031, 3, 15);
    assert_eq!(ensure_partitions(&pool, 3, now).await.unwrap(), 4);
    assert_eq!(ensure_partitions(&pool, 3, now).await.unwrap(), 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn ensure_partitions_creates_next_n_months(pool: PgPool) {
    ensure_partitions(&pool, 3, at(2031, 11, 15)).await.unwrap();
    let names: Vec<String> = month_partitions(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.0)
        .collect();
    for want in [
        "clicks_y2031m11",
        "clicks_y2031m12",
        "clicks_y2032m01",
        "clicks_y2032m02",
    ] {
        assert!(names.iter().any(|n| n == want), "missing {want}: {names:?}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn insert_routes_to_correct_partition(pool: PgPool) {
    let p = seed(&pool).await;
    ensure_partitions(&pool, 0, at(2031, 5, 1)).await.unwrap();
    let id = Uuid::now_v7();
    // 2031-05-31 23:30 UTC is still May in UTC whatever the session time zone.
    sqlx::query("SET TIME ZONE 'Asia/Tokyo'")
        .execute(&pool)
        .await
        .ok();
    click(
        &pool,
        p,
        id,
        Utc.with_ymd_and_hms(2031, 5, 31, 23, 30, 0).unwrap(),
    )
    .await;
    let part: String =
        sqlx::query_scalar("SELECT tableoid::regclass::text FROM clicks WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(part, "clicks_y2031m05");
}

#[sqlx::test(migrations = "./migrations")]
async fn out_of_range_goes_default_and_gauge_counts_it(pool: PgPool) {
    let p = seed(&pool).await;
    assert_eq!(default_rows(&pool).await.unwrap(), 0);
    click(&pool, p, Uuid::now_v7(), at(2090, 1, 1)).await;
    assert_eq!(default_rows(&pool).await.unwrap(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn ensure_moves_default_rows_before_attach(pool: PgPool) {
    let p = seed(&pool).await;
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    click(&pool, p, a, at(2091, 2, 3)).await;
    click(&pool, p, b, at(2091, 3, 3)).await;
    assert_eq!(default_rows(&pool).await.unwrap(), 2);
    assert_eq!(
        ensure_partitions(&pool, 0, at(2091, 2, 10)).await.unwrap(),
        1
    );
    assert_eq!(default_rows(&pool).await.unwrap(), 1, "only Feb moved");
    let part: String =
        sqlx::query_scalar("SELECT tableoid::regclass::text FROM clicks WHERE id = $1")
            .bind(a)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(part, "clicks_y2091m02");
}

#[sqlx::test(migrations = "./migrations")]
async fn replayed_click_dedups(pool: PgPool) {
    let p = seed(&pool).await;
    let c = Click {
        id: Uuid::now_v7(),
        program_id: p.0,
        affiliate_id: p.1,
        code: "c".into(),
        visitor_id: "v".into(),
        ts_ms: at(2031, 5, 5).timestamp_millis(),
        ip_hash: None,
        country: None,
        device: None,
        os: None,
        browser: None,
        referrer_host: None,
        landing_path: None,
        fraud_reason: None,
    };
    insert_clicks(&pool, std::slice::from_ref(&c))
        .await
        .unwrap();
    assert_eq!(insert_clicks(&pool, &[c]).await.unwrap(), 0);
    assert_eq!(count(&pool, "SELECT count(*) FROM clicks").await, 1);
}

/// Two programs' worth of rollup state so the watermark is defined.
async fn mark_rolled(pool: &PgPool, at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO rollup_state (program_id, ran_at) SELECT id, $1 FROM programs \
         ON CONFLICT (program_id) DO UPDATE SET ran_at = EXCLUDED.ran_at",
    )
    .bind(at)
    .execute(pool)
    .await
    .unwrap();
}

fn part(t: DateTime<Utc>) -> String {
    format!("clicks_y{:04}m{:02}", t.year(), t.month())
}

#[sqlx::test(migrations = "./migrations")]
async fn retention_drops_old_partition_keeps_rollups(pool: PgPool) {
    let p = seed(&pool).await;
    let (now, old, recent) = (Utc::now(), ago(8), ago(1));
    ensure_partitions(&pool, 0, old).await.unwrap();
    ensure_partitions(&pool, 0, recent).await.unwrap();
    click(&pool, p, Uuid::now_v7(), old).await;
    click(&pool, p, Uuid::now_v7(), recent).await;
    sqlx::query(
        "INSERT INTO daily_rollups (program_id, affiliate_id, day, dim, clicks) \
         VALUES ($1, $2, $3, 'total', 1)",
    )
    .bind(p.0)
    .bind(p.1)
    .bind(old.date_naive())
    .execute(&pool)
    .await
    .unwrap();
    mark_rolled(&pool, now).await;

    let r = drop_expired_partitions(&pool, 3, now).await.unwrap();
    assert_eq!(r.dropped, vec![part(old)]);
    assert_eq!(count(&pool, "SELECT count(*) FROM clicks").await, 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM daily_rollups").await, 1);
    let floor = retention_floor(&pool).await.unwrap().unwrap();
    assert_eq!(
        floor,
        NaiveDate::from_ymd_opt(old.year(), old.month(), 1).unwrap() + Months::new(1)
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM audit_log WHERE action = 'clicks.partition_dropped'"
        )
        .await,
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn retention_zero_keeps_everything(pool: PgPool) {
    let p = seed(&pool).await;
    let old = ago(30);
    ensure_partitions(&pool, 0, old).await.unwrap();
    click(&pool, p, Uuid::now_v7(), old).await;
    mark_rolled(&pool, Utc::now()).await;
    let r = drop_expired_partitions(&pool, 0, Utc::now()).await.unwrap();
    assert!(r.dropped.is_empty() && r.skipped.is_empty());
    assert_eq!(count(&pool, "SELECT count(*) FROM clicks").await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn retention_skips_partition_not_yet_rolled_up(pool: PgPool) {
    let p = seed(&pool).await;
    let (now, old) = (Utc::now(), ago(8));
    ensure_partitions(&pool, 0, old).await.unwrap();
    click(&pool, p, Uuid::now_v7(), old).await;
    let end = NaiveDate::from_ymd_opt(old.year(), old.month(), 1).unwrap() + Months::new(1);
    let end = Utc.from_utc_datetime(&end.and_hms_opt(0, 0, 0).unwrap());
    // Rollup last ran inside the partition's final day: that day may still be incomplete.
    mark_rolled(&pool, end - chrono::Duration::hours(1)).await;
    let r = drop_expired_partitions(&pool, 3, now).await.unwrap();
    assert!(r.dropped.is_empty());
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM clicks").await, 1);
    assert!(retention_floor(&pool).await.unwrap().is_none());

    // Once the rollup is a day past the partition end it goes.
    mark_rolled(&pool, end + chrono::Duration::days(1)).await;
    let r = drop_expired_partitions(&pool, 3, now).await.unwrap();
    assert_eq!(r.dropped.len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn retention_skips_when_a_program_never_rolled_up(pool: PgPool) {
    seed(&pool).await;
    ensure_partitions(&pool, 0, ago(8)).await.unwrap();
    let r = drop_expired_partitions(&pool, 3, Utc::now()).await.unwrap();
    assert!(r.dropped.is_empty());
    assert!(r.skipped[0].1.contains("no rollup"));
}

#[sqlx::test(migrations = "./migrations")]
async fn retention_logs_skipped_reason(pool: PgPool) {
    seed(&pool).await;
    let old = ago(8);
    ensure_partitions(&pool, 0, old).await.unwrap();
    drop_expired_partitions(&pool, 3, Utc::now()).await.unwrap();
    let reason: String = sqlx::query_scalar(
        "SELECT diff->>'reason' FROM audit_log WHERE action = 'clicks.partition_skipped' \
         AND entity_id = $1",
    )
    .bind(part(old))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(reason.contains("no rollup"), "{reason}");
}

#[sqlx::test(migrations = "./migrations")]
async fn retention_never_drops_recent_even_if_configured_tiny(pool: PgPool) {
    seed(&pool).await;
    // Last month's partition ended days ago at most: younger than MIN_RETENTION_DAYS can
    // only be dropped when months is tiny AND the end is old enough; 1 month must keep it.
    let now = Utc::now();
    ensure_partitions(&pool, 0, ago(1)).await.unwrap();
    mark_rolled(&pool, now + chrono::Duration::days(60)).await;
    let r = drop_expired_partitions(&pool, 1, now).await.unwrap();
    assert!(
        !r.dropped.contains(&part(now)),
        "current month must survive"
    );
    assert!(!r.dropped.contains(&part(ago(1))) || now.day() > 7);
}
