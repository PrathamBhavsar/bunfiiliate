//! Payout batches: `draft -> approved -> paid`, or `draft -> cancelled`.
//!
//! Creating a batch posts nothing: money only moves at `mark_paid` (payable -> cash).
//! Each payout pays the affiliate's whole unbatched payable, so clawbacks from reversed paid
//! conversions are netted automatically.

use bf_core::{Currency, Money, PayoutEvent, PayoutStatus};
use bf_db::payouts::{self, CsvRow};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::error::AppError;
use crate::worker::post;

/// Locked batch row.
struct Batch {
    status: PayoutStatus,
    currency: Currency,
}

fn status(s: &str) -> Option<PayoutStatus> {
    Some(match s {
        "draft" => PayoutStatus::Draft,
        "approved" => PayoutStatus::Approved,
        "paid" => PayoutStatus::Paid,
        "cancelled" => PayoutStatus::Cancelled,
        _ => return None,
    })
}

fn invalid<T>(_: T) -> AppError {
    AppError::Conflict("payout batch cannot take this action")
}

async fn lock(c: &mut PgConnection, id: Uuid) -> Result<Batch, AppError> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT b.status::text, p.currency::text FROM payout_batches b \
         JOIN programs p ON p.id = b.program_id WHERE b.id = $1 FOR UPDATE OF b",
    )
    .bind(id)
    .fetch_optional(&mut *c)
    .await?;
    let (st, cur) = row.ok_or(AppError::NotFound("payout batch not found"))?;
    match (status(&st), Currency::new(&cur)) {
        (Some(status), Some(currency)) => Ok(Batch { status, currency }),
        _ => Err(AppError::Internal),
    }
}

/// Draft one payout per affiliate owed at least their minimum, and claim their approved
/// conversions. `None` when nobody is owed.
pub async fn create_batch(
    pool: &PgPool,
    program_id: Uuid,
    created_by: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let mut tx = pool.begin().await?;
    // NO KEY UPDATE keeps ingest (FK checks) running but serializes batch creation, so two
    // callers cannot both claim the same payable.
    sqlx::query_scalar::<_, i32>("SELECT 1 FROM programs WHERE id = $1 FOR NO KEY UPDATE")
        .bind(program_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound("program not found"))?;
    let owed = payouts::owed(&mut tx, program_id).await?;
    if owed.is_empty() {
        return Ok(None);
    }
    let batch = Uuid::now_v7();
    sqlx::query("INSERT INTO payout_batches (id, program_id, created_by) VALUES ($1, $2, $3)")
        .bind(batch)
        .bind(program_id)
        .bind(created_by)
        .execute(&mut *tx)
        .await?;
    for o in owed {
        let payout = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO payouts (id, batch_id, affiliate_id, amount_cents) VALUES ($1, $2, $3, $4)",
        )
        .bind(payout)
        .bind(batch)
        .bind(o.affiliate_id)
        .bind(o.amount_cents)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE conversions SET payout_id = $1 \
             WHERE program_id = $2 AND affiliate_id = $3 AND status = 'approved' AND payout_id IS NULL",
        )
        .bind(payout)
        .bind(program_id)
        .bind(o.affiliate_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Some(batch))
}

pub async fn approve_batch(pool: &PgPool, batch: Uuid, approved_by: Uuid) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let b = lock(&mut tx, batch).await?;
    b.status.transition(PayoutEvent::Approve).map_err(invalid)?;
    sqlx::query("UPDATE payout_batches SET status = 'approved', approved_by = $2 WHERE id = $1")
        .bind(batch)
        .bind(approved_by)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE payouts SET status = 'approved' WHERE batch_id = $1")
        .bind(batch)
        .execute(&mut *tx)
        .await?;
    Ok(tx.commit().await?)
}

/// Only a draft batch can be cancelled; its conversions go back to the payable pool.
pub async fn cancel_batch(pool: &PgPool, batch: Uuid) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let b = lock(&mut tx, batch).await?;
    b.status.transition(PayoutEvent::Cancel).map_err(invalid)?;
    sqlx::query("UPDATE payout_batches SET status = 'cancelled' WHERE id = $1")
        .bind(batch)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE payouts SET status = 'cancelled' WHERE batch_id = $1")
        .bind(batch)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE conversions SET payout_id = NULL \
         WHERE payout_id IN (SELECT id FROM payouts WHERE batch_id = $1)",
    )
    .bind(batch)
    .execute(&mut *tx)
    .await?;
    Ok(tx.commit().await?)
}

/// Pay an approved batch: post payable -> cash per payout and turn its claimed conversions
/// `paid`. A second call finds the batch paid and is rejected.
pub async fn mark_paid(pool: &PgPool, batch: Uuid) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let b = lock(&mut tx, batch).await?;
    // The batch itself moves no money; its payouts do.
    b.status
        .transition(PayoutEvent::MarkPaid(Money::new(0, b.currency)))
        .map_err(invalid)?;
    let rows: Vec<(Uuid, Uuid, i64, String)> = sqlx::query_as(
        "SELECT id, affiliate_id, amount_cents, status::text FROM payouts \
         WHERE batch_id = $1 ORDER BY id FOR UPDATE",
    )
    .bind(batch)
    .fetch_all(&mut *tx)
    .await?;
    let program: Uuid = sqlx::query_scalar("SELECT program_id FROM payout_batches WHERE id = $1")
        .bind(batch)
        .fetch_one(&mut *tx)
        .await?;
    for (payout, affiliate, cents, st) in rows {
        let st = status(&st).ok_or(AppError::Internal)?;
        let (_, txn) = st
            .transition(PayoutEvent::MarkPaid(Money::new(cents, b.currency)))
            .map_err(invalid)?;
        let txn_id = match txn {
            Some(t) => post(&mut tx, &t, program, affiliate, (None, Some(payout))).await?,
            None => None,
        };
        sqlx::query("UPDATE payouts SET status = 'paid', ledger_txn_id = $2 WHERE id = $1")
            .bind(payout)
            .bind(txn_id)
            .execute(&mut *tx)
            .await?;
        // Approved -> Paid; a conversion reversed since the batch was drafted stays reversed.
        sqlx::query(
            "UPDATE conversions SET status = 'paid' WHERE payout_id = $1 AND status = 'approved'",
        )
        .bind(payout)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE payout_batches SET status = 'paid', paid_at = now() WHERE id = $1")
        .bind(batch)
        .execute(&mut *tx)
        .await?;
    Ok(tx.commit().await?)
}

/// CSV of a batch for the bank/payout tool.
pub async fn batch_csv(pool: &PgPool, batch: Uuid) -> Result<String, AppError> {
    let mut conn = pool.acquire().await?;
    sqlx::query_scalar::<_, i32>("SELECT 1 FROM payout_batches WHERE id = $1")
        .bind(batch)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(AppError::NotFound("payout batch not found"))?;
    Ok(export_csv(&payouts::batch_rows(&mut conn, batch).await?))
}

/// `email,amount,currency` with a header; amounts are decimal with two places.
pub fn export_csv(rows: &[CsvRow]) -> String {
    let mut out = String::from("email,amount,currency\n");
    for r in rows {
        out.push_str(&format!(
            "{},{}.{:02},{}\n",
            field(&r.email),
            r.amount_cents / 100,
            r.amount_cents % 100,
            field(&r.currency)
        ));
    }
    out
}

/// RFC 4180 quoting, only when needed.
fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_only_when_needed() {
        let row = |email: &str, cents| CsvRow {
            email: email.into(),
            amount_cents: cents,
            currency: "USD".into(),
        };
        assert_eq!(
            export_csv(&[row("a@x.io", 5), row("b,\"c\"@x.io", 123_456)]),
            "email,amount,currency\na@x.io,0.05,USD\n\"b,\"\"c\"\"@x.io\",1234.56,USD\n"
        );
    }
}
