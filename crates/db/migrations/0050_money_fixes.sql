-- Provider business object a conversion was paid for (Stripe invoice or payment intent, ...).
-- Stripe sends both invoice.paid and checkout.session.completed for one subscription payment;
-- the unique key makes the second one a no-op instead of a second commission.
ALTER TABLE conversions ADD COLUMN provider_ref text;
CREATE UNIQUE INDEX conversions_provider_ref_idx ON conversions (program_id, provider_ref)
    WHERE provider_ref IS NOT NULL;

-- Lets the analytics rollup find back-dated rows.
ALTER TABLE conversions ADD COLUMN created_at timestamptz NOT NULL DEFAULT now();

-- A reversal that arrives before its sale waits and is retried, with a cap.
ALTER TABLE inbound_events ADD COLUMN attempts integer NOT NULL DEFAULT 0;
ALTER TABLE inbound_events ADD COLUMN retry_at timestamptz;

-- Where the money goes, fixed when the payout is drafted.
ALTER TABLE payouts ADD COLUMN payout_email text;

-- One bad row must not break the approve tick for every tenant. NOT VALID: rows written
-- before this migration are not checked, every new or changed row is.
ALTER TABLE programs ADD CONSTRAINT programs_hold_days_max CHECK (hold_days <= 365) NOT VALID;
ALTER TABLE programs ADD CONSTRAINT programs_cookie_days_max CHECK (cookie_days <= 365) NOT VALID;

-- When each program's analytics were last rebuilt, so status changes and back-dated
-- conversions since then are folded in.
CREATE TABLE rollup_state (
    program_id uuid PRIMARY KEY REFERENCES programs (id) ON DELETE CASCADE,
    ran_at     timestamptz NOT NULL
);
