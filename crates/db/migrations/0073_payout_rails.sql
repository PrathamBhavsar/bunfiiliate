-- Payout rails: provider dispatch state per payout line, and sealed per-workspace credentials.
-- `payouts.status` stays the ledger-governing state; `rail_state` tracks the provider call:
-- pending -> submitted -> paid | failed. The ledger posts only when a line is confirmed paid.
ALTER TABLE payouts ADD COLUMN rail text NOT NULL DEFAULT 'manual_csv'
    CHECK (rail IN ('manual_csv', 'paypal_payouts', 'wise', 'mock'));
ALTER TABLE payouts ADD COLUMN rail_state text
    CHECK (rail_state IN ('pending', 'submitted', 'paid', 'failed'));
ALTER TABLE payouts ADD COLUMN rail_receipt jsonb;
ALTER TABLE payouts ADD COLUMN rail_status text;
ALTER TABLE payouts ADD COLUMN rail_error text;

CREATE TABLE payout_rail_credentials (
    id           uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    rail         text NOT NULL CHECK (rail IN ('paypal_payouts', 'wise')),
    sandbox      boolean NOT NULL DEFAULT true,
    -- Sealed JSON object of provider credentials (Vault::seal_col).
    creds        text NOT NULL,
    creds_enc_at timestamptz,
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, rail)
);
