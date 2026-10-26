use bf_db::ledger::{Account, NewEntry, balance_for_affiliate, insert_txn};
use sqlx::PgPool;
use uuid::Uuid;

async fn seed(pool: &PgPool) -> (Uuid, Uuid) {
    let (ws, user, aff, prog) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO workspaces (id, name, slug) VALUES ($1, 'w', 'w')")
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
    sqlx::query(
        "INSERT INTO programs (id, workspace_id, name, currency, public_key, webhook_secret) \
         VALUES ($1, $2, 'p', 'USD', 'pk', 's')",
    )
    .bind(prog)
    .bind(ws)
    .execute(pool)
    .await
    .unwrap();
    (prog, aff)
}

fn entry(account: Account, prog: Uuid, aff: Option<Uuid>, amount_cents: i64) -> NewEntry {
    NewEntry {
        account,
        program_id: prog,
        affiliate_id: aff,
        amount_cents,
        currency: "USD".into(),
        conversion_id: None,
        payout_id: None,
    }
}

/// Conversion posting: debit expense, credit aff_pending.
fn conversion(prog: Uuid, aff: Uuid, cents: i64) -> Vec<NewEntry> {
    vec![
        entry(Account::Expense, prog, None, cents),
        entry(Account::AffPending, prog, Some(aff), -cents),
    ]
}

#[sqlx::test]
async fn append_only_rejects_update_and_delete(pool: PgPool) {
    let (prog, aff) = seed(&pool).await;
    insert_txn(&pool, Uuid::now_v7(), &conversion(prog, aff, 500))
        .await
        .unwrap();

    let upd = sqlx::query("UPDATE ledger_entries SET amount_cents = amount_cents + 1")
        .execute(&pool)
        .await;
    assert!(upd.unwrap_err().to_string().contains("append-only"));

    let del = sqlx::query("DELETE FROM ledger_entries")
        .execute(&pool)
        .await;
    assert!(del.unwrap_err().to_string().contains("append-only"));

    let trunc = sqlx::query("TRUNCATE ledger_entries").execute(&pool).await;
    assert!(trunc.unwrap_err().to_string().contains("append-only"));

    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
}

#[sqlx::test]
async fn unbalanced_txn_fails_at_commit(pool: PgPool) {
    let (prog, aff) = seed(&pool).await;
    let bad = vec![
        entry(Account::Expense, prog, None, 500),
        entry(Account::AffPending, prog, Some(aff), -499),
    ];
    let err = insert_txn(&pool, Uuid::now_v7(), &bad).await.unwrap_err();
    assert!(err.to_string().contains("unbalanced"), "{err}");

    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test]
async fn balances_view_tracks_postings(pool: PgPool) {
    let (prog, aff) = seed(&pool).await;
    let zero = balance_for_affiliate(&pool, prog, aff).await.unwrap();
    assert_eq!((zero.pending_cents, zero.payable_cents), (0, 0));

    insert_txn(&pool, Uuid::now_v7(), &conversion(prog, aff, 1000))
        .await
        .unwrap();
    insert_txn(&pool, Uuid::now_v7(), &conversion(prog, aff, 250))
        .await
        .unwrap();
    let b = balance_for_affiliate(&pool, prog, aff).await.unwrap();
    assert_eq!((b.pending_cents, b.payable_cents), (1250, 0));

    // Approve 1000: debit aff_pending, credit aff_payable.
    let approve = vec![
        entry(Account::AffPending, prog, Some(aff), 1000),
        entry(Account::AffPayable, prog, Some(aff), -1000),
    ];
    insert_txn(&pool, Uuid::now_v7(), &approve).await.unwrap();
    let b = balance_for_affiliate(&pool, prog, aff).await.unwrap();
    assert_eq!((b.pending_cents, b.payable_cents), (250, 1000));

    // Payout 1000: debit aff_payable, credit cash.
    let paid = vec![
        entry(Account::AffPayable, prog, Some(aff), 1000),
        entry(Account::Cash, prog, None, -1000),
    ];
    insert_txn(&pool, Uuid::now_v7(), &paid).await.unwrap();
    let b = balance_for_affiliate(&pool, prog, aff).await.unwrap();
    assert_eq!((b.pending_cents, b.payable_cents), (250, 0));
}

#[sqlx::test]
async fn migrate_fn_is_idempotent(pool: PgPool) {
    // sqlx::test already applied migrations; running again is a no-op.
    bf_db::migrate(&pool).await.unwrap();
}
