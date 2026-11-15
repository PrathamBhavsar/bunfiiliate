-- Affiliate-initiated payout requests. A request only asks; the payout is sized from the
-- ledger balance when an admin approves it.
CREATE TYPE payout_request_status AS ENUM ('pending', 'approved', 'rejected', 'paid');

CREATE TABLE payout_requests (
    id               uuid PRIMARY KEY,
    affiliate_id     uuid NOT NULL REFERENCES affiliates (id),
    program_id       uuid NOT NULL REFERENCES programs (id),
    requested_cents  bigint NOT NULL CHECK (requested_cents > 0),
    currency         char(3) NOT NULL,
    status           payout_request_status NOT NULL DEFAULT 'pending',
    rejection_reason text,
    payout_id        uuid REFERENCES payouts (id),
    created_at       timestamptz NOT NULL DEFAULT now(),
    decided_at       timestamptz,
    decided_by       uuid REFERENCES users (id)
);
CREATE INDEX payout_requests_affiliate_idx ON payout_requests (affiliate_id, status);
CREATE INDEX payout_requests_program_idx ON payout_requests (program_id, status);
-- One open request per affiliate and program, enforced by the database so concurrent
-- submits cannot both win.
CREATE UNIQUE INDEX payout_requests_one_pending
    ON payout_requests (affiliate_id, program_id) WHERE status = 'pending';
