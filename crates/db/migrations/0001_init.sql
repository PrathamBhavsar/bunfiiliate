CREATE EXTENSION IF NOT EXISTS citext;

CREATE TYPE user_role AS ENUM ('super_admin', 'admin', 'affiliate');
CREATE TYPE affiliate_status AS ENUM ('pending', 'active', 'suspended');
CREATE TYPE commission_kind AS ENUM ('percent', 'flat');
CREATE TYPE commission_duration AS ENUM ('one_month', 'x_months', 'lifetime');
CREATE TYPE attribution_model AS ENUM ('first_touch', 'last_touch');
CREATE TYPE conversion_status AS ENUM ('pending', 'approved', 'rejected', 'reversed', 'paid');
CREATE TYPE payout_status AS ENUM ('draft', 'approved', 'paid', 'cancelled');
CREATE TYPE event_source AS ENUM ('generic', 'stripe', 'paddle');
CREATE TYPE ledger_account AS ENUM ('expense', 'aff_pending', 'aff_payable', 'cash');
CREATE TYPE rollup_dim AS ENUM ('total', 'country', 'os', 'browser', 'device', 'referrer');

-- All ids are generated in Rust (UUIDv7); no column defaults.

CREATE TABLE workspaces (
    id         uuid PRIMARY KEY,
    name       text NOT NULL,
    slug       text NOT NULL UNIQUE,
    settings   jsonb NOT NULL DEFAULT '{}'
);

CREATE TABLE users (
    id            uuid PRIMARY KEY,
    workspace_id  uuid REFERENCES workspaces (id),
    email         citext NOT NULL UNIQUE,
    password_hash text NOT NULL,
    role          user_role NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX users_workspace_idx ON users (workspace_id);

CREATE TABLE refresh_tokens (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    family_id   uuid NOT NULL,
    token_hash  bytea NOT NULL UNIQUE,
    expires_at  timestamptz NOT NULL,
    revoked_at  timestamptz,
    replaced_by uuid REFERENCES refresh_tokens (id)
);
CREATE INDEX refresh_tokens_user_idx ON refresh_tokens (user_id);
CREATE INDEX refresh_tokens_family_idx ON refresh_tokens (family_id);

CREATE TABLE programs (
    id                uuid PRIMARY KEY,
    workspace_id      uuid NOT NULL REFERENCES workspaces (id),
    name              text NOT NULL,
    currency          char(3) NOT NULL,
    attribution_model attribution_model NOT NULL DEFAULT 'last_touch',
    cookie_days       integer NOT NULL DEFAULT 30 CHECK (cookie_days > 0),
    auto_approve      boolean NOT NULL DEFAULT false,
    hold_days         integer NOT NULL DEFAULT 0 CHECK (hold_days >= 0),
    default_plan_id   uuid, -- FK added below (circular with commission_plans)
    public_key        text NOT NULL UNIQUE,
    webhook_secret    text NOT NULL,
    stripe_secret     text,
    paddle_secret     text,
    created_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX programs_workspace_idx ON programs (workspace_id);

CREATE TABLE program_sites (
    program_id uuid NOT NULL REFERENCES programs (id) ON DELETE CASCADE,
    domain     text NOT NULL,
    PRIMARY KEY (program_id, domain)
);

CREATE TABLE conversion_types (
    id         uuid PRIMARY KEY,
    program_id uuid NOT NULL REFERENCES programs (id) ON DELETE CASCADE,
    key        text NOT NULL,
    name       text NOT NULL,
    UNIQUE (program_id, key)
);

CREATE TABLE commission_plans (
    id                uuid PRIMARY KEY,
    program_id        uuid NOT NULL REFERENCES programs (id) ON DELETE CASCADE,
    conversion_type_id uuid REFERENCES conversion_types (id),
    kind              commission_kind NOT NULL,
    rate_bps          integer CHECK (rate_bps BETWEEN 0 AND 10000),
    amount_cents      bigint CHECK (amount_cents >= 0),
    duration          commission_duration NOT NULL DEFAULT 'lifetime',
    months            integer CHECK (months > 0),
    CHECK (kind <> 'percent' OR rate_bps IS NOT NULL),
    CHECK (kind <> 'flat' OR amount_cents IS NOT NULL),
    CHECK (duration <> 'x_months' OR months IS NOT NULL)
);
CREATE INDEX commission_plans_program_idx ON commission_plans (program_id);
CREATE INDEX commission_plans_type_idx ON commission_plans (conversion_type_id);

ALTER TABLE programs
    ADD CONSTRAINT programs_default_plan_fk
    FOREIGN KEY (default_plan_id) REFERENCES commission_plans (id);

CREATE TABLE affiliates (
    id               uuid PRIMARY KEY,
    workspace_id     uuid NOT NULL REFERENCES workspaces (id),
    user_id          uuid NOT NULL UNIQUE REFERENCES users (id),
    status           affiliate_status NOT NULL DEFAULT 'pending',
    payout_email     citext,
    payout_min_cents bigint NOT NULL DEFAULT 0 CHECK (payout_min_cents >= 0),
    created_at       timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX affiliates_workspace_idx ON affiliates (workspace_id);

CREATE TABLE enrollments (
    affiliate_id     uuid NOT NULL REFERENCES affiliates (id) ON DELETE CASCADE,
    program_id       uuid NOT NULL REFERENCES programs (id) ON DELETE CASCADE,
    plan_override_id uuid REFERENCES commission_plans (id),
    PRIMARY KEY (affiliate_id, program_id)
);
CREATE INDEX enrollments_program_idx ON enrollments (program_id);

CREATE TABLE referral_codes (
    code         text PRIMARY KEY,
    affiliate_id uuid NOT NULL REFERENCES affiliates (id) ON DELETE CASCADE,
    program_id   uuid NOT NULL REFERENCES programs (id) ON DELETE CASCADE,
    landing_url  text
);
CREATE INDEX referral_codes_affiliate_idx ON referral_codes (affiliate_id);
CREATE INDEX referral_codes_program_idx ON referral_codes (program_id);

CREATE TABLE clicks (
    id             uuid PRIMARY KEY,
    program_id     uuid NOT NULL REFERENCES programs (id),
    affiliate_id   uuid NOT NULL REFERENCES affiliates (id),
    code           text NOT NULL,
    visitor_id     text NOT NULL,
    ts             timestamptz NOT NULL,
    ip_hash        bytea,
    country        char(2),
    device         text,
    os             text,
    browser        text,
    referrer_host  text,
    landing_path   text,
    fraud_reason   text
);
CREATE INDEX clicks_ts_brin ON clicks USING brin (ts);
CREATE INDEX clicks_affiliate_idx ON clicks (affiliate_id);
CREATE INDEX clicks_visitor_idx ON clicks (program_id, visitor_id);

CREATE TABLE inbound_events (
    id           uuid PRIMARY KEY,
    program_id   uuid NOT NULL REFERENCES programs (id),
    source       event_source NOT NULL,
    external_id  text NOT NULL,
    payload      jsonb NOT NULL,
    received_at  timestamptz NOT NULL DEFAULT now(),
    processed_at timestamptz,
    error        text,
    UNIQUE (program_id, source, external_id)
);
CREATE INDEX inbound_events_unprocessed_idx ON inbound_events (received_at) WHERE processed_at IS NULL;

CREATE TABLE customers (
    id             uuid PRIMARY KEY,
    program_id     uuid NOT NULL REFERENCES programs (id),
    external_id    text NOT NULL,
    affiliate_id   uuid REFERENCES affiliates (id),
    first_click_id uuid,
    attributed_at  timestamptz,
    UNIQUE (program_id, external_id)
);
CREATE INDEX customers_affiliate_idx ON customers (affiliate_id);

CREATE TABLE conversions (
    id                 uuid PRIMARY KEY,
    program_id         uuid NOT NULL REFERENCES programs (id),
    customer_id        uuid NOT NULL REFERENCES customers (id),
    affiliate_id       uuid NOT NULL REFERENCES affiliates (id),
    conversion_type_id uuid REFERENCES conversion_types (id),
    external_id        text NOT NULL,
    amount_cents       bigint NOT NULL,
    commission_cents   bigint NOT NULL,
    currency           char(3) NOT NULL,
    status             conversion_status NOT NULL DEFAULT 'pending',
    occurred_at        timestamptz NOT NULL,
    approved_at        timestamptz,
    reversed_at        timestamptz,
    UNIQUE (program_id, external_id)
);
CREATE INDEX conversions_affiliate_idx ON conversions (affiliate_id, status);
CREATE INDEX conversions_customer_idx ON conversions (customer_id);
CREATE INDEX conversions_pending_idx ON conversions (occurred_at) WHERE status = 'pending';

CREATE TABLE payout_batches (
    id          uuid PRIMARY KEY,
    program_id  uuid NOT NULL REFERENCES programs (id),
    status      payout_status NOT NULL DEFAULT 'draft',
    created_by  uuid NOT NULL REFERENCES users (id),
    approved_by uuid REFERENCES users (id),
    paid_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX payout_batches_program_idx ON payout_batches (program_id);

CREATE TABLE payouts (
    id            uuid PRIMARY KEY,
    batch_id      uuid NOT NULL REFERENCES payout_batches (id) ON DELETE CASCADE,
    affiliate_id  uuid NOT NULL REFERENCES affiliates (id),
    amount_cents  bigint NOT NULL,
    status        payout_status NOT NULL DEFAULT 'draft',
    ledger_txn_id uuid -- txn_id is not unique in ledger_entries, so no FK
);
CREATE INDEX payouts_batch_idx ON payouts (batch_id);
CREATE INDEX payouts_affiliate_idx ON payouts (affiliate_id);

-- Double-entry convention: debit > 0, credit < 0. Every txn sums to zero.
CREATE TABLE ledger_entries (
    id           uuid PRIMARY KEY,
    txn_id       uuid NOT NULL,
    account      ledger_account NOT NULL,
    program_id   uuid NOT NULL REFERENCES programs (id),
    affiliate_id uuid REFERENCES affiliates (id),
    amount_cents bigint NOT NULL,
    currency     char(3) NOT NULL,
    conversion_id uuid REFERENCES conversions (id),
    payout_id    uuid REFERENCES payouts (id),
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ledger_entries_txn_idx ON ledger_entries (txn_id);
CREATE INDEX ledger_entries_affiliate_idx ON ledger_entries (program_id, affiliate_id, account)
    WHERE affiliate_id IS NOT NULL;
CREATE INDEX ledger_entries_conversion_idx ON ledger_entries (conversion_id) WHERE conversion_id IS NOT NULL;
CREATE INDEX ledger_entries_payout_idx ON ledger_entries (payout_id) WHERE payout_id IS NOT NULL;

CREATE FUNCTION ledger_entries_reject_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'ledger_entries is append-only (% rejected)', TG_OP
        USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

CREATE TRIGGER ledger_entries_append_only
    BEFORE UPDATE OR DELETE ON ledger_entries
    FOR EACH ROW EXECUTE FUNCTION ledger_entries_reject_mutation();

CREATE TRIGGER ledger_entries_no_truncate
    BEFORE TRUNCATE ON ledger_entries
    FOR EACH STATEMENT EXECUTE FUNCTION ledger_entries_reject_mutation();

CREATE FUNCTION ledger_entries_check_balanced() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    total bigint;
BEGIN
    SELECT sum(amount_cents) INTO total FROM ledger_entries WHERE txn_id = NEW.txn_id;
    IF total <> 0 THEN
        RAISE EXCEPTION 'ledger txn % is unbalanced (sum = %)', NEW.txn_id, total
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER ledger_entries_balanced
    AFTER INSERT ON ledger_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION ledger_entries_check_balanced();

-- Liability accounts are credit-normal, so balances are the negated sums.
CREATE VIEW affiliate_balances AS
SELECT program_id,
       affiliate_id,
       currency,
       -COALESCE(sum(amount_cents) FILTER (WHERE account = 'aff_pending'), 0)::bigint AS pending_cents,
       -COALESCE(sum(amount_cents) FILTER (WHERE account = 'aff_payable'), 0)::bigint AS payable_cents
FROM ledger_entries
WHERE affiliate_id IS NOT NULL
GROUP BY program_id, affiliate_id, currency;

CREATE TABLE outbox (
    id           bigserial PRIMARY KEY,
    topic        text NOT NULL,
    payload      jsonb NOT NULL,
    available_at timestamptz NOT NULL DEFAULT now(),
    attempts     integer NOT NULL DEFAULT 0,
    locked_until timestamptz,
    done_at      timestamptz
);
CREATE INDEX outbox_ready_idx ON outbox (available_at) WHERE done_at IS NULL;

CREATE TABLE webhook_endpoints (
    id           uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    url          text NOT NULL,
    secret       text NOT NULL,
    events       text[] NOT NULL DEFAULT '{}',
    active       boolean NOT NULL DEFAULT true
);
CREATE INDEX webhook_endpoints_workspace_idx ON webhook_endpoints (workspace_id);

CREATE TABLE webhook_deliveries (
    id              uuid PRIMARY KEY,
    endpoint_id     uuid NOT NULL REFERENCES webhook_endpoints (id) ON DELETE CASCADE,
    outbox_id       bigint NOT NULL REFERENCES outbox (id),
    attempt         integer NOT NULL DEFAULT 1,
    status_code     integer,
    next_attempt_at timestamptz
);
CREATE INDEX webhook_deliveries_endpoint_idx ON webhook_deliveries (endpoint_id);
CREATE INDEX webhook_deliveries_outbox_idx ON webhook_deliveries (outbox_id);

CREATE TABLE daily_rollups (
    program_id       uuid NOT NULL REFERENCES programs (id) ON DELETE CASCADE,
    affiliate_id     uuid NOT NULL REFERENCES affiliates (id) ON DELETE CASCADE,
    day              date NOT NULL,
    dim              rollup_dim NOT NULL,
    dim_value        text NOT NULL DEFAULT '',
    clicks           bigint NOT NULL DEFAULT 0,
    conversions      bigint NOT NULL DEFAULT 0,
    revenue_cents    bigint NOT NULL DEFAULT 0,
    commission_cents bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (program_id, affiliate_id, day, dim, dim_value)
);
CREATE INDEX daily_rollups_day_idx ON daily_rollups (program_id, day);

CREATE TABLE notifications (
    id      uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind    text NOT NULL,
    body    text NOT NULL,
    read_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX notifications_user_idx ON notifications (user_id, created_at DESC);

CREATE TABLE email_templates (
    id           uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    key          text NOT NULL,
    subject      text NOT NULL,
    body         text NOT NULL,
    UNIQUE (workspace_id, key)
);

CREATE TABLE contact_requests (
    id           uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    program_id   uuid REFERENCES programs (id),
    name         text NOT NULL,
    email        citext NOT NULL,
    message      text NOT NULL,
    status       text NOT NULL DEFAULT 'new',
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX contact_requests_workspace_idx ON contact_requests (workspace_id, created_at DESC);

CREATE TABLE audit_log (
    id           uuid PRIMARY KEY,
    workspace_id uuid REFERENCES workspaces (id),
    actor_id     uuid REFERENCES users (id),
    action       text NOT NULL,
    entity       text NOT NULL,
    entity_id    text NOT NULL,
    diff         jsonb,
    at           timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_workspace_idx ON audit_log (workspace_id, at DESC);
