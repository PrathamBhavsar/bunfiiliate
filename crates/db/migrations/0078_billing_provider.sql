-- B10: billing provider rails. Webhook secrets are sealed (Vault) and never returned.
ALTER TABLE billing_plans ADD COLUMN lemonsqueezy_variant_id text;
-- updated_at of the newest provider event applied; older events are ignored.
ALTER TABLE subscriptions ADD COLUMN provider_event_at timestamptz;

CREATE TABLE billing_providers (
    id             uuid PRIMARY KEY,
    provider       text NOT NULL UNIQUE CHECK (provider IN ('mock', 'lemonsqueezy')),
    webhook_secret text NOT NULL,
    secret_enc_at  timestamptz,
    updated_at     timestamptz NOT NULL DEFAULT now()
);

-- Provider event ids already applied; a replay is a no-op.
CREATE TABLE billing_events (
    provider    text NOT NULL CHECK (provider IN ('mock', 'lemonsqueezy')),
    event_id    text NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, event_id)
);
