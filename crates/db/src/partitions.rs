//! Monthly `clicks` partitions (UTC): creation, the catch-all row count, and retention.
//! Schema and `bf_ensure_clicks_partition` live in migration 0080; see docs/ops/partitioning.md.

use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, TimeZone, Utc};
use sqlx::PgPool;

/// Retention never drops data younger than this, whatever the configured months: the rollup
/// rebuilds the last day (`analytics::LOOKBACK_DAYS`), so a younger partition may still be read.
pub const MIN_RETENTION_DAYS: i64 = 7;

fn month_start(d: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year(), d.month(), 1).expect("first of month")
}

fn utc_midnight(d: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight"))
}

/// Create the partitions for the month of `now` and the next `months_ahead`. Idempotent;
/// returns how many were created.
pub async fn ensure_partitions(
    pool: &PgPool,
    months_ahead: u32,
    now: DateTime<Utc>,
) -> Result<u32, sqlx::Error> {
    let first = month_start(now.date_naive());
    let mut created = 0;
    for i in 0..=months_ahead {
        let month = first + Months::new(i);
        let made: bool = sqlx::query_scalar("SELECT bf_ensure_clicks_partition($1)")
            .bind(month)
            .fetch_one(pool)
            .await?;
        created += u32::from(made);
    }
    Ok(created)
}

/// Rows in the catch-all partition (`bf_clicks_default_rows`); should be 0.
pub async fn default_rows(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT bf_clicks_default_rows()")
        .fetch_one(pool)
        .await
}

/// `(name, month start)` of every month partition, oldest first.
pub async fn month_partitions(pool: &PgPool) -> Result<Vec<(String, NaiveDate)>, sqlx::Error> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid \
         WHERE i.inhparent = 'clicks'::regclass AND c.relname ~ '^clicks_y[0-9]{4}m[0-9]{2}$' \
         ORDER BY c.relname",
    )
    .fetch_all(pool)
    .await?;
    Ok(names
        .into_iter()
        .filter_map(|n| {
            let y: i32 = n.get(8..12)?.parse().ok()?;
            let m: u32 = n.get(13..15)?.parse().ok()?;
            Some((n, NaiveDate::from_ymd_opt(y, m, 1)?))
        })
        .collect())
}

#[derive(Debug, Default)]
pub struct Retention {
    pub dropped: Vec<String>,
    /// `(partition, reason)` for partitions old enough to drop but kept.
    pub skipped: Vec<(String, String)>,
}

/// Drop month partitions that end before `now - months` (and before `now - MIN_RETENTION_DAYS`),
/// but only once every program's rollup has run past the partition's end plus one day, so no
/// click is lost to analytics. `months == 0` keeps everything. Never touches `daily_rollups`;
/// records the dropped boundary in `clicks_retention` so rollups never rebuild those days.
/// Skips and drops are written to `audit_log`.
pub async fn drop_expired_partitions(
    pool: &PgPool,
    months: u32,
    now: DateTime<Utc>,
) -> Result<Retention, sqlx::Error> {
    let mut out = Retention::default();
    if months == 0 {
        return Ok(out);
    }
    let cutoff = (now - Months::new(months)).min(now - Duration::days(MIN_RETENTION_DAYS));
    // (programs without a rollup yet, oldest rollup run)
    let (unrolled, watermark): (i64, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE s.program_id IS NULL), min(s.ran_at) \
         FROM programs p LEFT JOIN rollup_state s ON s.program_id = p.id",
    )
    .fetch_one(pool)
    .await?;
    for (name, month) in month_partitions(pool).await? {
        let hi = utc_midnight(month + Months::new(1));
        if hi > cutoff {
            continue;
        }
        let reason = if unrolled > 0 {
            Some(format!("{unrolled} program(s) have no rollup yet"))
        } else {
            watermark.filter(|w| *w < hi + Duration::days(1)).map(|w| {
                format!(
                    "rollup watermark {} is before partition end {} + 1 day",
                    w.to_rfc3339(),
                    hi.to_rfc3339()
                )
            })
        };
        let mut tx = pool.begin().await?;
        if let Some(reason) = reason {
            sqlx::query(
                "INSERT INTO audit_log (id, action, entity, entity_id, diff) \
                 VALUES (gen_random_uuid(), 'clicks.partition_skipped', 'clicks_partition', $1, \
                         jsonb_build_object('reason', $2::text))",
            )
            .bind(&name)
            .bind(&reason)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            out.skipped.push((name, reason));
            continue;
        }
        // The name matched a strict pattern above; quoting is belt and braces.
        sqlx::query(&format!("ALTER TABLE clicks DETACH PARTITION \"{name}\""))
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("DROP TABLE \"{name}\""))
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO clicks_retention (floor_day, singleton) VALUES ($1, true) \
             ON CONFLICT (singleton) DO UPDATE \
             SET floor_day = GREATEST(clicks_retention.floor_day, EXCLUDED.floor_day)",
        )
        .bind(hi.date_naive())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO audit_log (id, action, entity, entity_id, diff) \
             VALUES (gen_random_uuid(), 'clicks.partition_dropped', 'clicks_partition', $1, \
                     jsonb_build_object('floor_day', $2::text))",
        )
        .bind(&name)
        .bind(hi.date_naive().to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        out.dropped.push(name);
    }
    Ok(out)
}

/// First day rollups may rebuild (None = no retention drop has happened).
pub async fn retention_floor(pool: &PgPool) -> Result<Option<NaiveDate>, sqlx::Error> {
    sqlx::query_scalar("SELECT floor_day FROM clicks_retention")
        .fetch_optional(pool)
        .await
}

/// Same as [`retention_floor`], inside an open transaction.
pub async fn retention_floor_tx(
    tx: &mut sqlx::PgConnection,
) -> Result<Option<NaiveDate>, sqlx::Error> {
    sqlx::query_scalar("SELECT floor_day FROM clicks_retention")
        .fetch_optional(tx)
        .await
}
