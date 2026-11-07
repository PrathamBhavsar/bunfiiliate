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
pub async fn owed(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    program_id: Uuid,
) -> Result<Vec<Owed>, sqlx::Error> {
    sqlx::query_as(
        "SELECT affiliate_id, amount_cents FROM ( \
           SELECT a.id AS affiliate_id, a.payout_min_cents, \
                  b.payable_cents - COALESCE((SELECT sum(p.amount_cents) FROM payouts p \
                    JOIN payout_batches pb ON pb.id = p.batch_id \
                    WHERE p.affiliate_id = a.id AND pb.program_id = $1 \
                      AND p.status IN ('draft', 'approved')), 0)::bigint AS amount_cents \
           FROM affiliate_balances b JOIN affiliates a ON a.id = b.affiliate_id \
           WHERE b.program_id = $1 AND a.workspace_id = $2 AND a.status = 'active') o \
         WHERE amount_cents > 0 AND amount_cents >= payout_min_cents \
         ORDER BY affiliate_id",
    )
    .bind(program_id)
    .bind(workspace_id)
    .fetch_all(conn)
    .await
}

/// Payout lines of a batch: the email snapshotted when it was drafted (the live payout email,
/// then the account email, for payouts that predate the snapshot), amount, program currency.
/// Payouts shrunk to nothing by reversals are left out: there is nothing to send.
pub async fn batch_rows(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    batch_id: Uuid,
) -> Result<Vec<CsvRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT COALESCE(p.payout_email, a.payout_email, u.email)::text AS email, p.amount_cents, \
                pr.currency::text AS currency \
         FROM payouts p JOIN payout_batches b ON b.id = p.batch_id \
           JOIN programs pr ON pr.id = b.program_id \
           JOIN affiliates a ON a.id = p.affiliate_id JOIN users u ON u.id = a.user_id \
         WHERE p.batch_id = $1 AND pr.workspace_id = $2 AND p.amount_cents > 0 \
         ORDER BY 1, p.id",
    )
    .bind(batch_id)
    .bind(workspace_id)
    .fetch_all(conn)
    .await
}

/// What one affiliate could be paid right now: payable minus open (draft/approved) payouts.
/// May be zero or negative (clawbacks); the caller applies the minimum. Active affiliates only.
pub async fn owed_for(
    conn: &mut PgConnection,
    program_id: Uuid,
    affiliate_id: Uuid,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT b.payable_cents - COALESCE((SELECT sum(p.amount_cents) FROM payouts p \
                JOIN payout_batches pb ON pb.id = p.batch_id \
                WHERE p.affiliate_id = a.id AND pb.program_id = $1 \
                  AND p.status IN ('draft', 'approved')), 0)::bigint \
         FROM affiliate_balances b JOIN affiliates a ON a.id = b.affiliate_id \
         WHERE b.program_id = $1 AND a.id = $2 AND a.status = 'active'",
    )
    .bind(program_id)
    .bind(affiliate_id)
    .fetch_optional(conn)
    .await
}
