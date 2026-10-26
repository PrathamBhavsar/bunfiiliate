//! Tracker queries: config snapshot rows and the click batch insert.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Postgres NOTIFY channel the API fires after code/program changes.
pub const CONFIG_CHANNEL: &str = "bf_config";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CodeRow {
    pub code: String,
    pub program_id: Uuid,
    pub affiliate_id: Uuid,
    pub landing_url: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProgramKeyRow {
    pub public_key: String,
    pub program_id: Uuid,
    pub webhook_secret: String,
    pub stripe_secret: Option<String>,
    pub paddle_secret: Option<String>,
}

pub async fn load_codes(pool: &PgPool) -> Result<Vec<CodeRow>, sqlx::Error> {
    // `landing_url` is nullable in the schema; a code without one cannot redirect.
    sqlx::query_as(
        "SELECT code, program_id, affiliate_id, landing_url FROM referral_codes \
         WHERE landing_url IS NOT NULL",
    )
    .fetch_all(pool)
    .await
}

pub async fn load_program_keys(pool: &PgPool) -> Result<Vec<ProgramKeyRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT public_key, id AS program_id, webhook_secret, stripe_secret, paddle_secret \
         FROM programs",
    )
    .fetch_all(pool)
    .await
}

/// One recorded click; also the spool payload (JSON). `ts_ms` is unix millis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Click {
    pub id: Uuid,
    pub program_id: Uuid,
    pub affiliate_id: Uuid,
    pub code: String,
    pub visitor_id: String,
    pub ts_ms: i64,
    pub ip_hash: Option<Vec<u8>>,
    pub country: Option<String>,
    pub device: Option<String>,
    pub os: Option<String>,
    pub browser: Option<String>,
    pub referrer_host: Option<String>,
    pub landing_path: Option<String>,
}

/// Batch insert; replays and retries are no-ops on the primary key.
pub async fn insert_clicks(pool: &PgPool, clicks: &[Click]) -> Result<u64, sqlx::Error> {
    macro_rules! col {
        ($f:ident) => {
            clicks.iter().map(|c| c.$f.clone()).collect::<Vec<_>>()
        };
    }
    let res = sqlx::query(
        "INSERT INTO clicks (id, program_id, affiliate_id, code, visitor_id, ts, ip_hash, \
                             country, device, os, browser, referrer_host, landing_path) \
         SELECT id, program_id, affiliate_id, code, visitor_id, \
                TIMESTAMPTZ 'epoch' + ts_ms * INTERVAL '1 millisecond', \
                ip_hash, country, device, os, browser, referrer_host, landing_path \
         FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[], $4::text[], $5::text[], $6::bigint[], \
                     $7::bytea[], $8::text[], $9::text[], $10::text[], $11::text[], \
                     $12::text[], $13::text[]) \
              AS t(id, program_id, affiliate_id, code, visitor_id, ts_ms, ip_hash, country, \
                   device, os, browser, referrer_host, landing_path) \
         ON CONFLICT DO NOTHING",
    )
    .bind(col!(id))
    .bind(col!(program_id))
    .bind(col!(affiliate_id))
    .bind(col!(code))
    .bind(col!(visitor_id))
    .bind(col!(ts_ms))
    .bind(col!(ip_hash))
    .bind(col!(country))
    .bind(col!(device))
    .bind(col!(os))
    .bind(col!(browser))
    .bind(col!(referrer_host))
    .bind(col!(landing_path))
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}
