-- B09: billing plans and per-workspace subscriptions. Limits are rows, NULL = unlimited.
CREATE TABLE billing_plans (
    id                 uuid PRIMARY KEY,
    key                text UNIQUE NOT NULL,
    name               text NOT NULL,
    price_cents        bigint NOT NULL CHECK (price_cents >= 0),
    currency           char(3) NOT NULL DEFAULT 'USD',
    max_programs       int CHECK (max_programs >= 0),
    max_affiliates     int CHECK (max_affiliates >= 0),
    max_monthly_clicks bigint CHECK (max_monthly_clicks >= 0),
    is_default         boolean NOT NULL DEFAULT false
);
CREATE UNIQUE INDEX billing_plans_one_default ON billing_plans (is_default) WHERE is_default;

CREATE TYPE subscription_status AS ENUM ('active', 'past_due', 'canceled');

CREATE TABLE subscriptions (
    workspace_id             uuid PRIMARY KEY REFERENCES workspaces (id) ON DELETE CASCADE,
    plan_id                  uuid NOT NULL REFERENCES billing_plans (id),
    provider                 text NOT NULL DEFAULT 'mock',
    provider_subscription_id text,
    status                   subscription_status NOT NULL DEFAULT 'active',
    current_period_end       timestamptz,
    updated_at               timestamptz NOT NULL DEFAULT now()
);

-- Ids are generated in Rust elsewhere; the seed row uses a fixed v7-shaped uuid.
INSERT INTO billing_plans (id, key, name, price_cents, is_default)
VALUES ('01900000-0000-7000-8000-000000000001', 'free', 'Free', 0, true)
ON CONFLICT DO NOTHING;

-- Backfill workspaces created before billing existed.
INSERT INTO subscriptions (workspace_id, plan_id)
SELECT w.id, (SELECT id FROM billing_plans WHERE is_default)
FROM workspaces w
ON CONFLICT DO NOTHING;
