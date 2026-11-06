//! The snapshot loader decrypts sealed program secrets and survives bad rows.

use bf_core::secrets::{KeyRing, Vault};
use sqlx::PgPool;
use tracker::snapshot::Snapshot;
use uuid::Uuid;

fn vault(kid: &str, byte: u8) -> Vault {
    Vault::new(KeyRing::from_pairs(kid, &[(kid, [byte; 32])]).unwrap())
}

async fn workspace(pool: &PgPool) -> Uuid {
    let ws = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', 'w')")
        .bind(ws)
        .execute(pool)
        .await
        .unwrap();
    ws
}

async fn program(
    pool: &PgPool,
    ws: Uuid,
    key: &str,
    id: Uuid,
    webhook: &str,
    stripe: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret, stripe_secret) \
         VALUES ($1, $2, $3, 'USD', $3, $4, $5)",
    )
    .bind(id)
    .bind(ws)
    .bind(key)
    .bind(webhook)
    .bind(stripe)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_decrypts_sealed_secrets(pool: PgPool) {
    let v = vault("k1", 1);
    let ws = workspace(&pool).await;
    let id = Uuid::now_v7();
    let wh = v.seal_col("programs", "webhook_secret", id, "whsec_plain");
    let st = v.seal_col("programs", "stripe_secret", id, "whsec_stripe");
    program(&pool, ws, "pk", id, &wh, Some(&st)).await;
    let snap = Snapshot::load(&pool, &v).await.unwrap();
    let p = &snap.programs["pk"];
    assert_eq!(p.webhook_secret, "whsec_plain");
    assert_eq!(p.stripe_secret.as_deref(), Some("whsec_stripe"));
    assert!(p.paddle_secret.is_none());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_skips_undecryptable_row(pool: PgPool) {
    let v = vault("k1", 1);
    let other = vault("k1", 2); // same kid, different key bytes: authentication fails
    let ws = workspace(&pool).await;
    let (good, bad) = (Uuid::now_v7(), Uuid::now_v7());
    program(
        &pool,
        ws,
        "good",
        good,
        &v.seal_col("programs", "webhook_secret", good, "g"),
        None,
    )
    .await;
    program(
        &pool,
        ws,
        "bad",
        bad,
        &other.seal_col("programs", "webhook_secret", bad, "b"),
        None,
    )
    .await;
    let before = tracker::metrics::SNAPSHOT_DECRYPT_FAIL.get();
    let snap = Snapshot::load(&pool, &v).await.unwrap();
    assert!(snap.programs.contains_key("good"));
    assert!(!snap.programs.contains_key("bad"));
    assert!(snap.broken.contains("bad"));
    assert!(tracker::metrics::SNAPSHOT_DECRYPT_FAIL.get() > before);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_accepts_legacy_plaintext(pool: PgPool) {
    let ws = workspace(&pool).await;
    program(
        &pool,
        ws,
        "old",
        Uuid::now_v7(),
        "whsec_legacy",
        Some("sk_legacy"),
    )
    .await;
    let snap = Snapshot::load(&pool, &vault("k1", 1)).await.unwrap();
    assert_eq!(snap.programs["old"].webhook_secret, "whsec_legacy");
    assert_eq!(
        snap.programs["old"].stripe_secret.as_deref(),
        Some("sk_legacy")
    );
}
