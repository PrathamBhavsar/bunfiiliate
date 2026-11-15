-- TOTP enrolment and recovery codes.
CREATE TABLE user_totp (
    user_id        uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- base32 secret sealed with Vault::seal_col, AAD "user_totp.secret_enc.<user_id>"
    secret_enc     text NOT NULL,
    secret_enc_at  timestamptz,
    confirmed_at   timestamptz,          -- NULL = pending enrol
    last_used_step bigint,
    created_at     timestamptz NOT NULL DEFAULT now()
);

-- code_hash is a salted argon2 PHC string, never a fast hash: codes carry ~50 bits.
CREATE TABLE user_recovery_codes (
    id         uuid PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_hash  text NOT NULL,
    used_at    timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX user_recovery_codes_user_idx ON user_recovery_codes (user_id) WHERE used_at IS NULL;
