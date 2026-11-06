//! Secrets at rest: loading the master keys and listing the protected columns.

use bf_core::secrets::{SecretsError, Vault};

/// Every `(table, column)` holding a credential. `api secrets reencrypt` and `check` walk this
/// list. A new secret column must be added here, sealed on write with `Vault::seal_col`, and
/// get a leak test.
pub const PROTECTED: &[(&str, &str)] = &[
    ("programs", "webhook_secret"),
    ("programs", "stripe_secret"),
    ("programs", "paddle_secret"),
    ("webhook_endpoints", "secret"),
];

/// Load the vault from `BF_MASTER_KEYS` / `BF_MASTER_KEY_FILE`.
///
/// Refuses to start without keys unless `BF_ALLOW_PLAINTEXT_SECRETS=1`, in which case secrets
/// are stored in plaintext and a warning is logged.
pub fn load_vault() -> Result<Vault, SecretsError> {
    let allow = matches!(
        std::env::var("BF_ALLOW_PLAINTEXT_SECRETS").as_deref(),
        Ok("1" | "true")
    );
    let v = Vault::from_env(allow)?;
    if v.ring().is_none() {
        tracing::warn!("BF_ALLOW_PLAINTEXT_SECRETS is set: secrets are stored in plaintext");
    }
    Ok(v)
}

use bf_core::secrets::KeyRing;
use sqlx::PgPool;
use uuid::Uuid;

/// The `*_enc_at` marker column set when a row's secrets have been sealed.
fn marker(table: &str) -> &'static str {
    match table {
        "programs" => "secrets_enc_at",
        "webhook_endpoints" => "secret_enc_at",
        other => panic!("no encryption marker column for {other}"),
    }
}

/// What a `reencrypt` or `check` pass found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Plaintext values that were (or, in a dry run, would be) sealed.
    pub encrypted: u64,
    /// Values sealed under a non-active key whose DEK was (or would be) re-wrapped.
    pub rewrapped: u64,
    /// Values that could not be opened (unknown key id, tampering, wrong row).
    pub failed: u64,
}

impl Report {
    pub fn pending(&self) -> u64 {
        self.encrypted + self.rewrapped
    }
}

const BATCH: i64 = 200;

/// Seal every plaintext protected value and re-wrap values sealed under an older key.
///
/// Works in batches of 200 rows, each in its own transaction with `FOR UPDATE SKIP LOCKED`, and
/// fires `bf_config` inside that transaction so the tracker reloads on commit. Idempotent: a
/// second run finds nothing to do. `dry_run` only counts.
pub async fn reencrypt(
    pool: &PgPool,
    ring: &KeyRing,
    dry_run: bool,
) -> Result<Report, sqlx::Error> {
    let run_id = Uuid::now_v7();
    if !dry_run {
        sqlx::query("INSERT INTO secret_rotation_runs (id, active_kid) VALUES ($1, $2)")
            .bind(run_id)
            .bind(ring.active_kid())
            .execute(pool)
            .await?;
    }
    let mut report = Report::default();
    for &(table, column) in PROTECTED {
        let mut after = Uuid::nil();
        loop {
            let mut tx = pool.begin().await?;
            let rows = pending_rows(&mut tx, table, column, ring, after, !dry_run).await?;
            let Some(&(last, _)) = rows.last() else { break };
            after = last;
            let mut changed = false;
            for (id, value) in rows {
                let aad = Vault::aad(table, column, id);
                let next = if !KeyRing::is_sealed(&value) {
                    report.encrypted += 1;
                    Some(ring.seal(value.as_bytes(), aad.as_bytes()))
                } else {
                    match ring.rewrap(&value, aad.as_bytes()) {
                        Ok(s) => {
                            report.rewrapped += 1;
                            Some(s)
                        }
                        Err(e) => {
                            report.failed += 1;
                            tracing::error!(table, column, row = %id, error = %e, "cannot rewrap");
                            None
                        }
                    }
                };
                if let (Some(next), false) = (next, dry_run) {
                    let m = marker(table);
                    sqlx::query(&format!(
                        "UPDATE {table} SET {column} = $2, {m} = now() WHERE id = $1"
                    ))
                    .bind(id)
                    .bind(next)
                    .execute(&mut *tx)
                    .await?;
                    changed = true;
                }
            }
            if changed {
                bf_db::tracker::notify_config(&mut *tx).await?;
            }
            tx.commit().await?;
        }
    }
    if !dry_run {
        sqlx::query(
            "UPDATE secret_rotation_runs SET finished_at = now(), rows_rewrapped = $2, \
             rows_encrypted = $3 WHERE id = $1",
        )
        .bind(run_id)
        .bind(report.rewrapped as i64)
        .bind(report.encrypted as i64)
        .execute(pool)
        .await?;
    }
    Ok(report)
}

/// Rows of one protected column still needing work, after `after`, up to one batch.
async fn pending_rows(
    tx: &mut sqlx::PgConnection,
    table: &str,
    column: &str,
    ring: &KeyRing,
    after: Uuid,
    lock: bool,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let lock = if lock { "FOR UPDATE SKIP LOCKED" } else { "" };
    sqlx::query_as(&format!(
        "SELECT id, {column} FROM {table} \
         WHERE id > $1 AND {column} IS NOT NULL AND {column} <> '' \
           AND NOT starts_with({column}, $2) \
         ORDER BY id LIMIT $3 {lock}"
    ))
    .bind(after)
    .bind(format!("bfenc1:{}:", ring.active_kid()))
    .bind(BATCH)
    .fetch_all(tx)
    .await
}

/// Count what `reencrypt` would still do; nonzero means plaintext or old-key rows remain.
pub async fn check(pool: &PgPool, ring: &KeyRing) -> Result<Report, sqlx::Error> {
    reencrypt_dry(pool, ring).await
}

async fn reencrypt_dry(pool: &PgPool, ring: &KeyRing) -> Result<Report, sqlx::Error> {
    let mut report = Report::default();
    for &(table, column) in PROTECTED {
        let mut conn = pool.acquire().await?;
        let mut after = Uuid::nil();
        loop {
            let rows = pending_rows(&mut conn, table, column, ring, after, false).await?;
            let Some(&(last, _)) = rows.last() else { break };
            after = last;
            for (id, value) in rows {
                if !KeyRing::is_sealed(&value) {
                    report.encrypted += 1;
                } else if ring
                    .open(&value, Vault::aad(table, column, id).as_bytes())
                    .is_ok()
                {
                    report.rewrapped += 1;
                } else {
                    report.failed += 1;
                }
            }
        }
    }
    Ok(report)
}
