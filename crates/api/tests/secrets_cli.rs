//! `api secrets reencrypt` and `check`.

use std::time::Duration;

use api::secrets::{check, reencrypt};
use bf_core::secrets::{KeyRing, Vault};
use bf_db::tracker::CONFIG_CHANNEL;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use uuid::Uuid;

fn ring(active: &str, ids: &[&str]) -> KeyRing {
    let pairs: Vec<(&str, [u8; 32])> = ids
        .iter()
        .map(|i| (*i, [i.as_bytes()[i.len() - 1]; 32]))
        .collect();
    KeyRing::from_pairs(active, &pairs).unwrap()
}

/// One legacy-plaintext program, one sealed under k1, and an endpoint sealed under k1.
async fn seed(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let k1 = Vault::new(ring("k1", &["k1"]));
    let ws = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', 'w')")
        .bind(ws)
        .execute(pool)
        .await
        .unwrap();
    let (plain, sealed, ep) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    for (id, key, wh, stripe) in [
        (
            plain,
            "p1",
            "whsec_plain".to_string(),
            Some("sk_plain".to_string()),
        ),
        (
            sealed,
            "p2",
            k1.seal_col("programs", "webhook_secret", sealed, "whsec_sealed"),
            None,
        ),
    ] {
        sqlx::query(
            "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret, stripe_secret) \
             VALUES ($1, $2, $3, 'USD', $3, $4, $5)",
        )
        .bind(id)
        .bind(ws)
        .bind(key)
        .bind(wh)
        .bind(stripe)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO webhook_endpoints (id, workspace_id, url, secret, events, active) VALUES ($1, $2, 'https://e.io/h', $3, '{}', true)")
        .bind(ep)
        .bind(ws)
        .bind(k1.seal_col("webhook_endpoints", "secret", ep, "whsec_ep"))
        .execute(pool)
        .await
        .unwrap();
    (plain, sealed, ep)
}

async fn col(pool: &PgPool, table: &str, col: &str, id: Uuid) -> Option<String> {
    sqlx::query_scalar(&format!("SELECT {col} FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reencrypt_encrypts_plaintext_and_rewraps(pool: PgPool) {
    let (plain, sealed, ep) = seed(&pool).await;
    let r2 = ring("k2", &["k1", "k2"]);
    let mut l = PgListener::connect_with(&pool).await.unwrap();
    l.listen(CONFIG_CHANNEL).await.unwrap();

    let report = reencrypt(&pool, &r2, false).await.unwrap();
    assert_eq!(
        (report.encrypted, report.rewrapped, report.failed),
        (2, 2, 0)
    );

    let v = Vault::new(ring("k2", &["k1", "k2"]));
    for (table, column, id, want) in [
        ("programs", "webhook_secret", plain, "whsec_plain"),
        ("programs", "stripe_secret", plain, "sk_plain"),
        ("programs", "webhook_secret", sealed, "whsec_sealed"),
        ("webhook_endpoints", "secret", ep, "whsec_ep"),
    ] {
        let raw = col(&pool, table, column, id).await.unwrap();
        assert!(raw.starts_with("bfenc1:k2:"), "{table}.{column}: {raw}");
        assert_eq!(v.open_col(table, column, id, &raw).unwrap(), want);
    }
    assert_eq!(col(&pool, "programs", "paddle_secret", plain).await, None);
    assert!(check(&pool, &r2).await.unwrap().pending() == 0);
    // the tracker is told to reload
    tokio::time::timeout(Duration::from_secs(2), l.recv())
        .await
        .expect("bf_config notified")
        .unwrap();
    // the run is recorded
    let (enc, wrapped): (i64, i64) =
        sqlx::query_as("SELECT rows_encrypted, rows_rewrapped FROM secret_rotation_runs")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((enc, wrapped), (2, 2));
    // a ring with only k2 still reads everything
    let only_k2 = Vault::new(ring("k2", &["k2"]));
    let raw = col(&pool, "programs", "webhook_secret", sealed)
        .await
        .unwrap();
    assert!(
        only_k2
            .open_col("programs", "webhook_secret", sealed, &raw)
            .is_ok()
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reencrypt_is_idempotent(pool: PgPool) {
    seed(&pool).await;
    let r = ring("k2", &["k1", "k2"]);
    reencrypt(&pool, &r, false).await.unwrap();
    let again = reencrypt(&pool, &r, false).await.unwrap();
    assert_eq!(again.pending(), 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reencrypt_dry_run_changes_nothing(pool: PgPool) {
    let (plain, sealed, _) = seed(&pool).await;
    let before = (
        col(&pool, "programs", "webhook_secret", plain).await,
        col(&pool, "programs", "webhook_secret", sealed).await,
    );
    let report = reencrypt(&pool, &ring("k2", &["k1", "k2"]), true)
        .await
        .unwrap();
    assert_eq!((report.encrypted, report.rewrapped), (2, 2));
    let after = (
        col(&pool, "programs", "webhook_secret", plain).await,
        col(&pool, "programs", "webhook_secret", sealed).await,
    );
    assert_eq!(before, after);
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM secret_rotation_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn check_flags_plaintext_and_old_keys(pool: PgPool) {
    seed(&pool).await;
    let r = ring("k2", &["k1", "k2"]);
    assert!(check(&pool, &r).await.unwrap().pending() > 0);
    reencrypt(&pool, &r, false).await.unwrap();
    assert_eq!(check(&pool, &r).await.unwrap().pending(), 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reencrypt_reports_rows_it_cannot_open(pool: PgPool) {
    seed(&pool).await;
    // k1 rows exist but the ring no longer holds k1.
    let report = reencrypt(&pool, &ring("k2", &["k2"]), false).await.unwrap();
    assert_eq!(report.failed, 2);
    assert_eq!(report.encrypted, 2);
}
