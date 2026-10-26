-- Approved conversions claimed by a payout; they turn `paid` when the payout is marked paid.
ALTER TABLE conversions ADD COLUMN payout_id uuid REFERENCES payouts (id);
CREATE INDEX conversions_payout_idx ON conversions (payout_id) WHERE payout_id IS NOT NULL;
