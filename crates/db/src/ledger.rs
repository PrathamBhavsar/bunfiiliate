//! Minimal ledger repo. Debit > 0, credit < 0; a txn must sum to zero.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "ledger_account", rename_all = "snake_case")]
pub enum Account {
    Expense,
    AffPending,
    AffPayable,
    Cash,
}

#[derive(Debug, Clone)]
pub struct NewEntry {
    pub account: Account,
    pub program_id: Uuid,
    pub affiliate_id: Option<Uuid>,
    pub amount_cents: i64,
    pub currency: String,
    pub conversion_id: Option<Uuid>,
    pub payout_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct Balance {
    pub pending_cents: i64,
    pub payable_cents: i64,
}

/// Insert all entries of one txn atomically. The deferred balance trigger
/// fires at commit, so an unbalanced txn returns an error and writes nothing.
pub async fn insert_txn(
    pool: &PgPool,
    txn_id: Uuid,
    entries: &[NewEntry],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    insert_entries(&mut tx, txn_id, entries).await?;
    tx.commit().await
}

/// Insert the entries of one txn on an open connection; the balance check runs
/// when the caller's transaction commits.
pub async fn insert_entries(
    conn: &mut PgConnection,
    txn_id: Uuid,
    entries: &[NewEntry],
) -> Result<(), sqlx::Error> {
    for e in entries {
        sqlx::query(
            "INSERT INTO ledger_entries \
             (id, txn_id, account, program_id, affiliate_id, amount_cents, currency, conversion_id, payout_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(Uuid::now_v7())
        .bind(txn_id)
        .bind(e.account)
        .bind(e.program_id)
        .bind(e.affiliate_id)
        .bind(e.amount_cents)
        .bind(&e.currency)
        .bind(e.conversion_id)
        .bind(e.payout_id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Balance for an affiliate in a program (zero if no entries yet).
pub async fn balance_for_affiliate(
    pool: &PgPool,
    program_id: Uuid,
    affiliate_id: Uuid,
) -> Result<Balance, sqlx::Error> {
    let row = sqlx::query_as::<_, Balance>(
        "SELECT pending_cents, payable_cents FROM affiliate_balances \
         WHERE program_id = $1 AND affiliate_id = $2",
    )
    .bind(program_id)
    .bind(affiliate_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or(Balance {
        pending_cents: 0,
        payable_cents: 0,
    }))
}
