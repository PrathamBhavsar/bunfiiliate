//! C05 spike: Postgres row-level security on `programs`. All tests are `#[ignore]`: the policy is
//! never applied by a migration (see brain/decisions/0004-rls.md).
//! Run: `cargo test -p db --test rls_spike -- --ignored` (needs a role that can CREATE ROLE).
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const SPIKE: &str = include_str!("../migrations/0060_rls_spike.sql.disabled");

async fn seed(pool: &PgPool) -> (Uuid, Uuid) {
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    for (ws, slug) in [(a, "a"), (b, "b")] {
        sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, $2, $2)")
            .bind(ws)
            .bind(slug)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret)
             VALUES ($1, $2, $3, 'USD', $4, 's')",
        )
        .bind(Uuid::now_v7())
        .bind(ws)
        .bind(slug)
        .bind(format!("pk_{slug}"))
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::raw_sql(SPIKE).execute(pool).await.unwrap();
    (a, b)
}

/// Starts a tx acting as the non-owner role with `app.ws` set for that tx only.
async fn as_app(pool: &PgPool, ws: Option<Uuid>) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE bf_app")
        .execute(&mut *tx)
        .await
        .unwrap();
    if let Some(ws) = ws {
        sqlx::query("SELECT set_config('app.ws', $1, true)")
            .bind(ws.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx
}

#[sqlx::test]
#[ignore]
async fn rls_blocks_cross_ws_select_without_app_filter(pool: PgPool) {
    let (a, _b) = seed(&pool).await;
    // A query with no workspace predicate sees only the session's workspace.
    let mut tx = as_app(&pool, Some(a)).await;
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM programs")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(names, vec!["a".to_string()]);
    // Writing a row for another workspace is rejected by WITH CHECK.
    let other: Uuid = sqlx::query_scalar("SELECT id FROM workspaces WHERE slug = 'b'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let err = sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret)
         VALUES ($1, $2, 'x', 'USD', 'pk_x', 's')",
    )
    .bind(Uuid::now_v7())
    .bind(other)
    .execute(&mut *tx)
    .await;
    assert!(err.is_err());
    drop(tx);
    // Unset app.ws fails closed.
    let mut tx = as_app(&pool, None).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM programs")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test]
#[ignore]
async fn rls_set_local_resets_after_commit(pool: PgPool) {
    let (a, _b) = seed(&pool).await;
    // One pooled connection, so the second transaction reuses the first one's session.
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
    sqlx::query("SET LOCAL ROLE bf_app")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.ws', $1, true)")
        .bind(a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM programs")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 1);
    tx.commit().await.unwrap();
    // Role and setting are both gone: back to the owner, and app.ws is empty or NULL.
    let ws: Option<String> = sqlx::query_scalar("SELECT current_setting('app.ws', true)")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(ws.is_none() || ws.as_deref() == Some(""));
    let user: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_ne!(user, "bf_app");
}

#[sqlx::test]
#[ignore]
async fn rls_owner_role_bypass_documented(pool: PgPool) {
    seed(&pool).await;
    // The owner (no FORCE ROW LEVEL SECURITY) sees every workspace with no app.ws set. This is the
    // bypass path for migrations, the tracker and the worker. It is also why RLS is defense in depth
    // only: any code path on the owner connection is unprotected.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM programs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
}
