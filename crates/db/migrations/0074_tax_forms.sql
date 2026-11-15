-- B05: tax form capture. Last 4 of the tax id only; a full TIN is never stored.
CREATE TYPE tax_form_kind AS ENUM ('w8', 'w9', 'none');

ALTER TABLE affiliates
    ADD COLUMN tax_form_kind tax_form_kind NOT NULL DEFAULT 'none',
    ADD COLUMN tax_legal_name text,
    ADD COLUMN tax_country char(2),
    ADD COLUMN tax_id_last4 text CHECK (tax_id_last4 ~ '^[0-9]{4}$'),
    ADD COLUMN tax_form_submitted_at timestamptz;
