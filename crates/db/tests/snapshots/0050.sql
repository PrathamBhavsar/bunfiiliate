--
-- PostgreSQL database dump
--


-- Dumped from database version 16.15 (Homebrew)
-- Dumped by pg_dump version 16.15 (Homebrew)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Name: citext; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS citext WITH SCHEMA public;


--
-- Name: EXTENSION citext; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION citext IS 'data type for case-insensitive character strings';


--
-- Name: affiliate_status; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.affiliate_status AS ENUM (
    'pending',
    'active',
    'suspended'
);


--
-- Name: attribution_model; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.attribution_model AS ENUM (
    'first_touch',
    'last_touch'
);


--
-- Name: commission_duration; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.commission_duration AS ENUM (
    'one_month',
    'x_months',
    'lifetime'
);


--
-- Name: commission_kind; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.commission_kind AS ENUM (
    'percent',
    'flat'
);


--
-- Name: conversion_status; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.conversion_status AS ENUM (
    'pending',
    'approved',
    'rejected',
    'reversed',
    'paid'
);


--
-- Name: event_source; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.event_source AS ENUM (
    'generic',
    'stripe',
    'paddle'
);


--
-- Name: ledger_account; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.ledger_account AS ENUM (
    'expense',
    'aff_pending',
    'aff_payable',
    'cash'
);


--
-- Name: payout_status; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.payout_status AS ENUM (
    'draft',
    'approved',
    'paid',
    'cancelled'
);


--
-- Name: rollup_dim; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.rollup_dim AS ENUM (
    'total',
    'country',
    'os',
    'browser',
    'device',
    'referrer'
);


--
-- Name: user_role; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.user_role AS ENUM (
    'super_admin',
    'admin',
    'affiliate'
);


--
-- Name: ledger_entries_check_balanced(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ledger_entries_check_balanced() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
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


--
-- Name: ledger_entries_reject_mutation(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ledger_entries_reject_mutation() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION 'ledger_entries is append-only (% rejected)', TG_OP
        USING ERRCODE = 'integrity_constraint_violation';
END;
$$;


SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: _sqlx_migrations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public._sqlx_migrations (
    version bigint NOT NULL,
    description text NOT NULL,
    installed_on timestamp with time zone DEFAULT now() NOT NULL,
    success boolean NOT NULL,
    checksum bytea NOT NULL,
    execution_time bigint NOT NULL
);


--
-- Name: ledger_entries; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ledger_entries (
    id uuid NOT NULL,
    txn_id uuid NOT NULL,
    account public.ledger_account NOT NULL,
    program_id uuid NOT NULL,
    affiliate_id uuid,
    amount_cents bigint NOT NULL,
    currency character(3) NOT NULL,
    conversion_id uuid,
    payout_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: affiliate_balances; Type: VIEW; Schema: public; Owner: -
--

CREATE VIEW public.affiliate_balances AS
 SELECT program_id,
    affiliate_id,
    currency,
    (- (COALESCE(sum(amount_cents) FILTER (WHERE (account = 'aff_pending'::public.ledger_account)), (0)::numeric))::bigint) AS pending_cents,
    (- (COALESCE(sum(amount_cents) FILTER (WHERE (account = 'aff_payable'::public.ledger_account)), (0)::numeric))::bigint) AS payable_cents
   FROM public.ledger_entries
  WHERE (affiliate_id IS NOT NULL)
  GROUP BY program_id, affiliate_id, currency;


--
-- Name: affiliates; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.affiliates (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    status public.affiliate_status DEFAULT 'pending'::public.affiliate_status NOT NULL,
    payout_email public.citext,
    payout_min_cents bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT affiliates_payout_min_cents_check CHECK ((payout_min_cents >= 0))
);


--
-- Name: audit_log; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.audit_log (
    id uuid NOT NULL,
    workspace_id uuid,
    actor_id uuid,
    action text NOT NULL,
    entity text NOT NULL,
    entity_id text NOT NULL,
    diff jsonb,
    at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: clicks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.clicks (
    id uuid NOT NULL,
    program_id uuid NOT NULL,
    affiliate_id uuid NOT NULL,
    code text NOT NULL,
    visitor_id text NOT NULL,
    ts timestamp with time zone NOT NULL,
    ip_hash bytea,
    country character(2),
    device text,
    os text,
    browser text,
    referrer_host text,
    landing_path text,
    fraud_reason text
);


--
-- Name: commission_plans; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.commission_plans (
    id uuid NOT NULL,
    program_id uuid NOT NULL,
    conversion_type_id uuid,
    kind public.commission_kind NOT NULL,
    rate_bps integer,
    amount_cents bigint,
    duration public.commission_duration DEFAULT 'lifetime'::public.commission_duration NOT NULL,
    months integer,
    CONSTRAINT commission_plans_amount_cents_check CHECK ((amount_cents >= 0)),
    CONSTRAINT commission_plans_check CHECK (((kind <> 'percent'::public.commission_kind) OR (rate_bps IS NOT NULL))),
    CONSTRAINT commission_plans_check1 CHECK (((kind <> 'flat'::public.commission_kind) OR (amount_cents IS NOT NULL))),
    CONSTRAINT commission_plans_check2 CHECK (((duration <> 'x_months'::public.commission_duration) OR (months IS NOT NULL))),
    CONSTRAINT commission_plans_months_check CHECK ((months > 0)),
    CONSTRAINT commission_plans_rate_bps_check CHECK (((rate_bps >= 0) AND (rate_bps <= 10000)))
);


--
-- Name: contact_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.contact_requests (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    program_id uuid,
    name text NOT NULL,
    email public.citext NOT NULL,
    message text NOT NULL,
    status text DEFAULT 'new'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: conversion_types; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.conversion_types (
    id uuid NOT NULL,
    program_id uuid NOT NULL,
    key text NOT NULL,
    name text NOT NULL
);


--
-- Name: conversions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.conversions (
    id uuid NOT NULL,
    program_id uuid NOT NULL,
    customer_id uuid NOT NULL,
    affiliate_id uuid NOT NULL,
    conversion_type_id uuid,
    external_id text NOT NULL,
    amount_cents bigint NOT NULL,
    commission_cents bigint NOT NULL,
    currency character(3) NOT NULL,
    status public.conversion_status DEFAULT 'pending'::public.conversion_status NOT NULL,
    occurred_at timestamp with time zone NOT NULL,
    approved_at timestamp with time zone,
    reversed_at timestamp with time zone,
    payout_id uuid,
    hold_reason text,
    provider_ref text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: customers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.customers (
    id uuid NOT NULL,
    program_id uuid NOT NULL,
    external_id text NOT NULL,
    affiliate_id uuid,
    first_click_id uuid,
    attributed_at timestamp with time zone
);


--
-- Name: daily_rollups; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.daily_rollups (
    program_id uuid NOT NULL,
    affiliate_id uuid NOT NULL,
    day date NOT NULL,
    dim public.rollup_dim NOT NULL,
    dim_value text DEFAULT ''::text NOT NULL,
    clicks bigint DEFAULT 0 NOT NULL,
    conversions bigint DEFAULT 0 NOT NULL,
    revenue_cents bigint DEFAULT 0 NOT NULL,
    commission_cents bigint DEFAULT 0 NOT NULL
);


--
-- Name: email_templates; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.email_templates (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    key text NOT NULL,
    subject text NOT NULL,
    body text NOT NULL
);


--
-- Name: enrollments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.enrollments (
    affiliate_id uuid NOT NULL,
    program_id uuid NOT NULL,
    plan_override_id uuid
);


--
-- Name: inbound_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inbound_events (
    id uuid NOT NULL,
    program_id uuid NOT NULL,
    source public.event_source NOT NULL,
    external_id text NOT NULL,
    payload jsonb NOT NULL,
    received_at timestamp with time zone DEFAULT now() NOT NULL,
    processed_at timestamp with time zone,
    error text,
    attempts integer DEFAULT 0 NOT NULL,
    retry_at timestamp with time zone
);


--
-- Name: notifications; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.notifications (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    kind text NOT NULL,
    body text NOT NULL,
    read_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: outbox; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.outbox (
    id bigint NOT NULL,
    topic text NOT NULL,
    payload jsonb NOT NULL,
    available_at timestamp with time zone DEFAULT now() NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    locked_until timestamp with time zone,
    done_at timestamp with time zone
);


--
-- Name: outbox_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.outbox_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: outbox_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.outbox_id_seq OWNED BY public.outbox.id;


--
-- Name: payout_batches; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.payout_batches (
    id uuid NOT NULL,
    program_id uuid NOT NULL,
    status public.payout_status DEFAULT 'draft'::public.payout_status NOT NULL,
    created_by uuid NOT NULL,
    approved_by uuid,
    paid_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: payouts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.payouts (
    id uuid NOT NULL,
    batch_id uuid NOT NULL,
    affiliate_id uuid NOT NULL,
    amount_cents bigint NOT NULL,
    status public.payout_status DEFAULT 'draft'::public.payout_status NOT NULL,
    ledger_txn_id uuid,
    payout_email text
);


--
-- Name: program_sites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.program_sites (
    program_id uuid NOT NULL,
    domain text NOT NULL
);


--
-- Name: programs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.programs (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    name text NOT NULL,
    currency character(3) NOT NULL,
    attribution_model public.attribution_model DEFAULT 'last_touch'::public.attribution_model NOT NULL,
    cookie_days integer DEFAULT 30 NOT NULL,
    auto_approve boolean DEFAULT false NOT NULL,
    hold_days integer DEFAULT 0 NOT NULL,
    default_plan_id uuid,
    public_key text NOT NULL,
    webhook_secret text NOT NULL,
    stripe_secret text,
    paddle_secret text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT programs_cookie_days_check CHECK ((cookie_days > 0)),
    CONSTRAINT programs_hold_days_check CHECK ((hold_days >= 0))
);


--
-- Name: referral_codes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.referral_codes (
    code text NOT NULL,
    affiliate_id uuid NOT NULL,
    program_id uuid NOT NULL,
    landing_url text
);


--
-- Name: refresh_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.refresh_tokens (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    family_id uuid NOT NULL,
    token_hash bytea NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    replaced_by uuid
);


--
-- Name: rollup_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.rollup_state (
    program_id uuid NOT NULL,
    ran_at timestamp with time zone NOT NULL
);


--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    id uuid NOT NULL,
    workspace_id uuid,
    email public.citext NOT NULL,
    password_hash text NOT NULL,
    role public.user_role NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: webhook_deliveries; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.webhook_deliveries (
    id uuid NOT NULL,
    endpoint_id uuid NOT NULL,
    outbox_id bigint NOT NULL,
    attempt integer DEFAULT 1 NOT NULL,
    status_code integer,
    next_attempt_at timestamp with time zone
);


--
-- Name: webhook_endpoints; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.webhook_endpoints (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    url text NOT NULL,
    secret text NOT NULL,
    events text[] DEFAULT '{}'::text[] NOT NULL,
    active boolean DEFAULT true NOT NULL
);


--
-- Name: workspaces; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workspaces (
    id uuid NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    settings jsonb DEFAULT '{}'::jsonb NOT NULL
);


--
-- Name: outbox id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.outbox ALTER COLUMN id SET DEFAULT nextval('public.outbox_id_seq'::regclass);


--
-- Data for Name: _sqlx_migrations; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public._sqlx_migrations (version, description, installed_on, success, checksum, execution_time) VALUES (1, 'init', '2026-01-01 05:30:00+05:30', true, '\xe63df3d1b2aff52854b9682e4e2ed10fa656003de23f6c4ca4462140a803912824a19881c3a8c71d7a546ad9d6c0cb4e', 0);
INSERT INTO public._sqlx_migrations (version, description, installed_on, success, checksum, execution_time) VALUES (18, 'conversion payout', '2026-01-01 05:30:00+05:30', true, '\x19e472ec6ebca1615ff377ab1a3ef113f6c985549fc6daac3898b3b4d604fbc2994b38b81cb4f61db9229a34f6d3aee4', 0);
INSERT INTO public._sqlx_migrations (version, description, installed_on, success, checksum, execution_time) VALUES (40, 'conversion hold', '2026-01-01 05:30:00+05:30', true, '\x6b4b919b9b3df1999121c8c94bcf2c37011f580bdfca12a83dc63d0955fd7933ecfb3e0aab6d1440390a4f011420f245', 0);
INSERT INTO public._sqlx_migrations (version, description, installed_on, success, checksum, execution_time) VALUES (50, 'money fixes', '2026-01-01 05:30:00+05:30', true, '\x6275c050c658d4ebc3220d0660ebd18575c962c1be88d455c2394ad18a602211a7d9ad605288a565e85bb7a5c5efea78', 0);


--
-- Data for Name: affiliates; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: audit_log; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: clicks; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: commission_plans; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: contact_requests; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: conversion_types; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: conversions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: customers; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: daily_rollups; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: email_templates; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: enrollments; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: inbound_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ledger_entries; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: notifications; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: outbox; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: payout_batches; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: payouts; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: program_sites; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: programs; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: referral_codes; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: refresh_tokens; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: rollup_state; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: users; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.users (id, workspace_id, email, password_hash, role, created_at) VALUES ('00000000-0000-0000-0000-0000000000b1', '00000000-0000-0000-0000-0000000000a1', 'seed@example.com', 'x', 'admin', '2026-11-07 13:36:09.058439+05:30');


--
-- Data for Name: webhook_deliveries; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: webhook_endpoints; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workspaces; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.workspaces (id, name, slug, settings) VALUES ('00000000-0000-0000-0000-0000000000a1', 'Seed WS', 'seed-ws', '{}');


--
-- Name: outbox_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.outbox_id_seq', 1, false);


--
-- Name: _sqlx_migrations _sqlx_migrations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public._sqlx_migrations
    ADD CONSTRAINT _sqlx_migrations_pkey PRIMARY KEY (version);


--
-- Name: affiliates affiliates_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.affiliates
    ADD CONSTRAINT affiliates_pkey PRIMARY KEY (id);


--
-- Name: affiliates affiliates_user_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.affiliates
    ADD CONSTRAINT affiliates_user_id_key UNIQUE (user_id);


--
-- Name: audit_log audit_log_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_log
    ADD CONSTRAINT audit_log_pkey PRIMARY KEY (id);


--
-- Name: clicks clicks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.clicks
    ADD CONSTRAINT clicks_pkey PRIMARY KEY (id);


--
-- Name: commission_plans commission_plans_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.commission_plans
    ADD CONSTRAINT commission_plans_pkey PRIMARY KEY (id);


--
-- Name: contact_requests contact_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.contact_requests
    ADD CONSTRAINT contact_requests_pkey PRIMARY KEY (id);


--
-- Name: conversion_types conversion_types_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversion_types
    ADD CONSTRAINT conversion_types_pkey PRIMARY KEY (id);


--
-- Name: conversion_types conversion_types_program_id_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversion_types
    ADD CONSTRAINT conversion_types_program_id_key_key UNIQUE (program_id, key);


--
-- Name: conversions conversions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversions
    ADD CONSTRAINT conversions_pkey PRIMARY KEY (id);


--
-- Name: conversions conversions_program_id_external_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversions
    ADD CONSTRAINT conversions_program_id_external_id_key UNIQUE (program_id, external_id);


--
-- Name: customers customers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.customers
    ADD CONSTRAINT customers_pkey PRIMARY KEY (id);


--
-- Name: customers customers_program_id_external_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.customers
    ADD CONSTRAINT customers_program_id_external_id_key UNIQUE (program_id, external_id);


--
-- Name: daily_rollups daily_rollups_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.daily_rollups
    ADD CONSTRAINT daily_rollups_pkey PRIMARY KEY (program_id, affiliate_id, day, dim, dim_value);


--
-- Name: email_templates email_templates_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_templates
    ADD CONSTRAINT email_templates_pkey PRIMARY KEY (id);


--
-- Name: email_templates email_templates_workspace_id_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_templates
    ADD CONSTRAINT email_templates_workspace_id_key_key UNIQUE (workspace_id, key);


--
-- Name: enrollments enrollments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.enrollments
    ADD CONSTRAINT enrollments_pkey PRIMARY KEY (affiliate_id, program_id);


--
-- Name: inbound_events inbound_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inbound_events
    ADD CONSTRAINT inbound_events_pkey PRIMARY KEY (id);


--
-- Name: inbound_events inbound_events_program_id_source_external_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inbound_events
    ADD CONSTRAINT inbound_events_program_id_source_external_id_key UNIQUE (program_id, source, external_id);


--
-- Name: ledger_entries ledger_entries_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ledger_entries
    ADD CONSTRAINT ledger_entries_pkey PRIMARY KEY (id);


--
-- Name: notifications notifications_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);


--
-- Name: outbox outbox_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.outbox
    ADD CONSTRAINT outbox_pkey PRIMARY KEY (id);


--
-- Name: payout_batches payout_batches_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payout_batches
    ADD CONSTRAINT payout_batches_pkey PRIMARY KEY (id);


--
-- Name: payouts payouts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payouts
    ADD CONSTRAINT payouts_pkey PRIMARY KEY (id);


--
-- Name: program_sites program_sites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.program_sites
    ADD CONSTRAINT program_sites_pkey PRIMARY KEY (program_id, domain);


--
-- Name: programs programs_cookie_days_max; Type: CHECK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE public.programs
    ADD CONSTRAINT programs_cookie_days_max CHECK ((cookie_days <= 365)) NOT VALID;


--
-- Name: programs programs_hold_days_max; Type: CHECK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE public.programs
    ADD CONSTRAINT programs_hold_days_max CHECK ((hold_days <= 365)) NOT VALID;


--
-- Name: programs programs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.programs
    ADD CONSTRAINT programs_pkey PRIMARY KEY (id);


--
-- Name: programs programs_public_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.programs
    ADD CONSTRAINT programs_public_key_key UNIQUE (public_key);


--
-- Name: referral_codes referral_codes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral_codes
    ADD CONSTRAINT referral_codes_pkey PRIMARY KEY (code);


--
-- Name: refresh_tokens refresh_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_pkey PRIMARY KEY (id);


--
-- Name: refresh_tokens refresh_tokens_token_hash_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_token_hash_key UNIQUE (token_hash);


--
-- Name: rollup_state rollup_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rollup_state
    ADD CONSTRAINT rollup_state_pkey PRIMARY KEY (program_id);


--
-- Name: users users_email_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_email_key UNIQUE (email);


--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);


--
-- Name: webhook_deliveries webhook_deliveries_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhook_deliveries
    ADD CONSTRAINT webhook_deliveries_pkey PRIMARY KEY (id);


--
-- Name: webhook_endpoints webhook_endpoints_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhook_endpoints
    ADD CONSTRAINT webhook_endpoints_pkey PRIMARY KEY (id);


--
-- Name: workspaces workspaces_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_pkey PRIMARY KEY (id);


--
-- Name: workspaces workspaces_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_slug_key UNIQUE (slug);


--
-- Name: affiliates_workspace_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX affiliates_workspace_idx ON public.affiliates USING btree (workspace_id);


--
-- Name: audit_log_workspace_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX audit_log_workspace_idx ON public.audit_log USING btree (workspace_id, at DESC);


--
-- Name: clicks_affiliate_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX clicks_affiliate_idx ON public.clicks USING btree (affiliate_id);


--
-- Name: clicks_ts_brin; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX clicks_ts_brin ON public.clicks USING brin (ts);


--
-- Name: clicks_visitor_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX clicks_visitor_idx ON public.clicks USING btree (program_id, visitor_id);


--
-- Name: commission_plans_program_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX commission_plans_program_idx ON public.commission_plans USING btree (program_id);


--
-- Name: commission_plans_type_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX commission_plans_type_idx ON public.commission_plans USING btree (conversion_type_id);


--
-- Name: contact_requests_workspace_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX contact_requests_workspace_idx ON public.contact_requests USING btree (workspace_id, created_at DESC);


--
-- Name: conversions_affiliate_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX conversions_affiliate_idx ON public.conversions USING btree (affiliate_id, status);


--
-- Name: conversions_customer_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX conversions_customer_idx ON public.conversions USING btree (customer_id);


--
-- Name: conversions_payout_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX conversions_payout_idx ON public.conversions USING btree (payout_id) WHERE (payout_id IS NOT NULL);


--
-- Name: conversions_pending_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX conversions_pending_idx ON public.conversions USING btree (occurred_at) WHERE (status = 'pending'::public.conversion_status);


--
-- Name: conversions_provider_ref_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX conversions_provider_ref_idx ON public.conversions USING btree (program_id, provider_ref) WHERE (provider_ref IS NOT NULL);


--
-- Name: customers_affiliate_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX customers_affiliate_idx ON public.customers USING btree (affiliate_id);


--
-- Name: daily_rollups_day_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX daily_rollups_day_idx ON public.daily_rollups USING btree (program_id, day);


--
-- Name: enrollments_program_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX enrollments_program_idx ON public.enrollments USING btree (program_id);


--
-- Name: inbound_events_unprocessed_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inbound_events_unprocessed_idx ON public.inbound_events USING btree (received_at) WHERE (processed_at IS NULL);


--
-- Name: ledger_entries_affiliate_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ledger_entries_affiliate_idx ON public.ledger_entries USING btree (program_id, affiliate_id, account) WHERE (affiliate_id IS NOT NULL);


--
-- Name: ledger_entries_conversion_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ledger_entries_conversion_idx ON public.ledger_entries USING btree (conversion_id) WHERE (conversion_id IS NOT NULL);


--
-- Name: ledger_entries_payout_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ledger_entries_payout_idx ON public.ledger_entries USING btree (payout_id) WHERE (payout_id IS NOT NULL);


--
-- Name: ledger_entries_txn_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ledger_entries_txn_idx ON public.ledger_entries USING btree (txn_id);


--
-- Name: notifications_user_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX notifications_user_idx ON public.notifications USING btree (user_id, created_at DESC);


--
-- Name: outbox_ready_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX outbox_ready_idx ON public.outbox USING btree (available_at) WHERE (done_at IS NULL);


--
-- Name: payout_batches_program_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX payout_batches_program_idx ON public.payout_batches USING btree (program_id);


--
-- Name: payouts_affiliate_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX payouts_affiliate_idx ON public.payouts USING btree (affiliate_id);


--
-- Name: payouts_batch_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX payouts_batch_idx ON public.payouts USING btree (batch_id);


--
-- Name: programs_workspace_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX programs_workspace_idx ON public.programs USING btree (workspace_id);


--
-- Name: referral_codes_affiliate_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX referral_codes_affiliate_idx ON public.referral_codes USING btree (affiliate_id);


--
-- Name: referral_codes_program_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX referral_codes_program_idx ON public.referral_codes USING btree (program_id);


--
-- Name: refresh_tokens_family_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX refresh_tokens_family_idx ON public.refresh_tokens USING btree (family_id);


--
-- Name: refresh_tokens_user_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX refresh_tokens_user_idx ON public.refresh_tokens USING btree (user_id);


--
-- Name: users_workspace_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_workspace_idx ON public.users USING btree (workspace_id);


--
-- Name: webhook_deliveries_endpoint_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX webhook_deliveries_endpoint_idx ON public.webhook_deliveries USING btree (endpoint_id);


--
-- Name: webhook_deliveries_outbox_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX webhook_deliveries_outbox_idx ON public.webhook_deliveries USING btree (outbox_id);


--
-- Name: webhook_endpoints_workspace_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX webhook_endpoints_workspace_idx ON public.webhook_endpoints USING btree (workspace_id);


--
-- Name: ledger_entries ledger_entries_append_only; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ledger_entries_append_only BEFORE DELETE OR UPDATE ON public.ledger_entries FOR EACH ROW EXECUTE FUNCTION public.ledger_entries_reject_mutation();


--
-- Name: ledger_entries ledger_entries_balanced; Type: TRIGGER; Schema: public; Owner: -
--

CREATE CONSTRAINT TRIGGER ledger_entries_balanced AFTER INSERT ON public.ledger_entries DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.ledger_entries_check_balanced();


--
-- Name: ledger_entries ledger_entries_no_truncate; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ledger_entries_no_truncate BEFORE TRUNCATE ON public.ledger_entries FOR EACH STATEMENT EXECUTE FUNCTION public.ledger_entries_reject_mutation();


--
-- Name: affiliates affiliates_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.affiliates
    ADD CONSTRAINT affiliates_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: affiliates affiliates_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.affiliates
    ADD CONSTRAINT affiliates_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);


--
-- Name: audit_log audit_log_actor_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_log
    ADD CONSTRAINT audit_log_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.users(id);


--
-- Name: audit_log audit_log_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_log
    ADD CONSTRAINT audit_log_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);


--
-- Name: clicks clicks_affiliate_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.clicks
    ADD CONSTRAINT clicks_affiliate_id_fkey FOREIGN KEY (affiliate_id) REFERENCES public.affiliates(id);


--
-- Name: clicks clicks_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.clicks
    ADD CONSTRAINT clicks_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id);


--
-- Name: commission_plans commission_plans_conversion_type_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.commission_plans
    ADD CONSTRAINT commission_plans_conversion_type_id_fkey FOREIGN KEY (conversion_type_id) REFERENCES public.conversion_types(id);


--
-- Name: commission_plans commission_plans_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.commission_plans
    ADD CONSTRAINT commission_plans_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id) ON DELETE CASCADE;


--
-- Name: contact_requests contact_requests_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.contact_requests
    ADD CONSTRAINT contact_requests_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id);


--
-- Name: contact_requests contact_requests_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.contact_requests
    ADD CONSTRAINT contact_requests_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE CASCADE;


--
-- Name: conversion_types conversion_types_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversion_types
    ADD CONSTRAINT conversion_types_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id) ON DELETE CASCADE;


--
-- Name: conversions conversions_affiliate_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversions
    ADD CONSTRAINT conversions_affiliate_id_fkey FOREIGN KEY (affiliate_id) REFERENCES public.affiliates(id);


--
-- Name: conversions conversions_conversion_type_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversions
    ADD CONSTRAINT conversions_conversion_type_id_fkey FOREIGN KEY (conversion_type_id) REFERENCES public.conversion_types(id);


--
-- Name: conversions conversions_customer_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversions
    ADD CONSTRAINT conversions_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES public.customers(id);


--
-- Name: conversions conversions_payout_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversions
    ADD CONSTRAINT conversions_payout_id_fkey FOREIGN KEY (payout_id) REFERENCES public.payouts(id);


--
-- Name: conversions conversions_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversions
    ADD CONSTRAINT conversions_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id);


--
-- Name: customers customers_affiliate_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.customers
    ADD CONSTRAINT customers_affiliate_id_fkey FOREIGN KEY (affiliate_id) REFERENCES public.affiliates(id);


--
-- Name: customers customers_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.customers
    ADD CONSTRAINT customers_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id);


--
-- Name: daily_rollups daily_rollups_affiliate_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.daily_rollups
    ADD CONSTRAINT daily_rollups_affiliate_id_fkey FOREIGN KEY (affiliate_id) REFERENCES public.affiliates(id) ON DELETE CASCADE;


--
-- Name: daily_rollups daily_rollups_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.daily_rollups
    ADD CONSTRAINT daily_rollups_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id) ON DELETE CASCADE;


--
-- Name: email_templates email_templates_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_templates
    ADD CONSTRAINT email_templates_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE CASCADE;


--
-- Name: enrollments enrollments_affiliate_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.enrollments
    ADD CONSTRAINT enrollments_affiliate_id_fkey FOREIGN KEY (affiliate_id) REFERENCES public.affiliates(id) ON DELETE CASCADE;


--
-- Name: enrollments enrollments_plan_override_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.enrollments
    ADD CONSTRAINT enrollments_plan_override_id_fkey FOREIGN KEY (plan_override_id) REFERENCES public.commission_plans(id);


--
-- Name: enrollments enrollments_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.enrollments
    ADD CONSTRAINT enrollments_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id) ON DELETE CASCADE;


--
-- Name: inbound_events inbound_events_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inbound_events
    ADD CONSTRAINT inbound_events_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id);


--
-- Name: ledger_entries ledger_entries_affiliate_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ledger_entries
    ADD CONSTRAINT ledger_entries_affiliate_id_fkey FOREIGN KEY (affiliate_id) REFERENCES public.affiliates(id);


--
-- Name: ledger_entries ledger_entries_conversion_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ledger_entries
    ADD CONSTRAINT ledger_entries_conversion_id_fkey FOREIGN KEY (conversion_id) REFERENCES public.conversions(id);


--
-- Name: ledger_entries ledger_entries_payout_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ledger_entries
    ADD CONSTRAINT ledger_entries_payout_id_fkey FOREIGN KEY (payout_id) REFERENCES public.payouts(id);


--
-- Name: ledger_entries ledger_entries_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ledger_entries
    ADD CONSTRAINT ledger_entries_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id);


--
-- Name: notifications notifications_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: payout_batches payout_batches_approved_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payout_batches
    ADD CONSTRAINT payout_batches_approved_by_fkey FOREIGN KEY (approved_by) REFERENCES public.users(id);


--
-- Name: payout_batches payout_batches_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payout_batches
    ADD CONSTRAINT payout_batches_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id);


--
-- Name: payout_batches payout_batches_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payout_batches
    ADD CONSTRAINT payout_batches_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id);


--
-- Name: payouts payouts_affiliate_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payouts
    ADD CONSTRAINT payouts_affiliate_id_fkey FOREIGN KEY (affiliate_id) REFERENCES public.affiliates(id);


--
-- Name: payouts payouts_batch_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payouts
    ADD CONSTRAINT payouts_batch_id_fkey FOREIGN KEY (batch_id) REFERENCES public.payout_batches(id) ON DELETE CASCADE;


--
-- Name: program_sites program_sites_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.program_sites
    ADD CONSTRAINT program_sites_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id) ON DELETE CASCADE;


--
-- Name: programs programs_default_plan_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.programs
    ADD CONSTRAINT programs_default_plan_fk FOREIGN KEY (default_plan_id) REFERENCES public.commission_plans(id);


--
-- Name: programs programs_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.programs
    ADD CONSTRAINT programs_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);


--
-- Name: referral_codes referral_codes_affiliate_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral_codes
    ADD CONSTRAINT referral_codes_affiliate_id_fkey FOREIGN KEY (affiliate_id) REFERENCES public.affiliates(id) ON DELETE CASCADE;


--
-- Name: referral_codes referral_codes_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral_codes
    ADD CONSTRAINT referral_codes_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id) ON DELETE CASCADE;


--
-- Name: refresh_tokens refresh_tokens_replaced_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_replaced_by_fkey FOREIGN KEY (replaced_by) REFERENCES public.refresh_tokens(id);


--
-- Name: refresh_tokens refresh_tokens_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: rollup_state rollup_state_program_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rollup_state
    ADD CONSTRAINT rollup_state_program_id_fkey FOREIGN KEY (program_id) REFERENCES public.programs(id) ON DELETE CASCADE;


--
-- Name: users users_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);


--
-- Name: webhook_deliveries webhook_deliveries_endpoint_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhook_deliveries
    ADD CONSTRAINT webhook_deliveries_endpoint_id_fkey FOREIGN KEY (endpoint_id) REFERENCES public.webhook_endpoints(id) ON DELETE CASCADE;


--
-- Name: webhook_deliveries webhook_deliveries_outbox_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhook_deliveries
    ADD CONSTRAINT webhook_deliveries_outbox_id_fkey FOREIGN KEY (outbox_id) REFERENCES public.outbox(id);


--
-- Name: webhook_endpoints webhook_endpoints_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhook_endpoints
    ADD CONSTRAINT webhook_endpoints_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE CASCADE;


--
-- PostgreSQL database dump complete
--


