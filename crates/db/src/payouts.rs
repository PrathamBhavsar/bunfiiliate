//! Payout queries that need no domain logic.

use sqlx::PgConnection;
use uuid::Uuid;

/// What a payout would pay an affiliate right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct Owed {
    pub affiliate_id: Uuid,
    pub amount_cents: i64,
}

/// One line of a batch export.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CsvRow {
    pub email: String,
    pub amount_cents: i64,
    pub currency: String,
}

/// Active affiliates whose payable, net of payouts still open (draft or approved), is
/// positive and at least their `payout_min_cents`. A negative payable (clawback) is never owed.
pub async fn owed(conn: &mut PgConnection, program_id: Uuid) -> Result<Vec<Owed>, sqlx::Error> {
    sqlx::query_as(
        "SELECT affiliate_id, amount_cents FROM ( \
           SELECT a.id AS affiliate_id, a.payout_min_cents, \
                  b.payable_cents - COALESCE((SELECT sum(p.amount_cents) FROM payouts p \
                    JOIN payout_batches pb ON pb.id = p.batch_id \
                    WHERE p.affiliate_id = a.id AND pb.program_id = $1 \
                      AND p.status IN ('draft', 'approved')), 0)::bigint AS amount_cents \
           FROM affiliate_balances b JOIN affiliates a ON a.id = b.affiliate_id \
           WHERE b.program_id = $1 AND a.status = 'active') o \
         WHERE amount_cents > 0 AND amount_cents >= payout_min_cents \
         ORDER BY affiliate_id",
    )
    .bind(program_id)
    .fetch_all(conn)
    .await
}

/// Payout lines of a batch: payout email (account email as fallback), amount, program currency.
pub async fn batch_rows(
    conn: &mut PgConnection,
    batch_id: Uuid,
) -> Result<Vec<CsvRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT COALESCE(a.payout_email, u.email)::text AS email, p.amount_cents, \
                pr.currency::text AS currency \
         FROM payouts p JOIN payout_batches b ON b.id = p.batch_id \
           JOIN programs pr ON pr.id = b.program_id \
           JOIN affiliates a ON a.id = p.affiliate_id JOIN users u ON u.id = a.user_id \
         WHERE p.batch_id = $1 ORDER BY 1, p.id",
    )
    .bind(batch_id)
    .fetch_all(conn)
    .await
}
