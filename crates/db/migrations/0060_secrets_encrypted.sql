-- Secret columns keep their names; a value becomes a 'bfenc1:...' ciphertext once sealed.
-- NULL *_enc_at means the row may still hold a legacy plaintext value.
ALTER TABLE programs ADD COLUMN secrets_enc_at timestamptz;
ALTER TABLE webhook_endpoints ADD COLUMN secret_enc_at timestamptz;

CREATE TABLE secret_rotation_runs (
    id             uuid PRIMARY KEY,
    started_at     timestamptz NOT NULL DEFAULT now(),
    finished_at    timestamptz,
    active_kid     text NOT NULL,
    rows_rewrapped bigint NOT NULL DEFAULT 0,
    rows_encrypted bigint NOT NULL DEFAULT 0
);
