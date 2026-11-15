CREATE TYPE contact_request_kind AS ENUM
    ('general_inquiry', 'technical_support', 'account_issue', 'payout_dispute');
ALTER TABLE contact_requests ADD COLUMN kind contact_request_kind NOT NULL DEFAULT 'general_inquiry';
ALTER TABLE contact_requests ADD COLUMN amount_cents bigint CHECK (amount_cents IS NULL OR amount_cents >= 0);
ALTER TABLE contact_requests ADD COLUMN currency char(3);
ALTER TABLE contact_requests ADD CONSTRAINT contact_amount_requires_currency
    CHECK ((amount_cents IS NULL) = (currency IS NULL));
