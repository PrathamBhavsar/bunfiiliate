# Plan B: live-feature parity, signup, billing, payouts

Extends `docs/PLAN.md` after Plan A (`T00..T60`, done). Same rules apply: smallest
complete change, clean-room (no code/schema text/copy from the live reference app —
it is read only for feature *identification*), `prod <- staging <- feat/*`,
`--no-ff` only, `make ci` green before any push.

**Hard rules for this plan specifically:**

- Workers are **gemini | kiro | opencode only**. Never Codex. Every task below is
  written self-contained for a mid-tier agent: exact files, exact schema, exact
  routes, exact assertions. No task assumes prior institutional knowledge beyond
  what's quoted here and in `brain/`.
- No email dependency. Notifications = in-app (have) + signed outbound webhooks
  (have) + optional Telegram/Discord webhook (new, off by default) + optional
  SMTP (have, already off by default without `SMTP_URL`). Nothing may *require*
  email to function.
- No paid services. Billing/payout integrations must have a real free tier or be
  self-hosted OSS, selected behind a trait with a `Mock` impl so tests never hit
  the network.
- Commit messages for this plan **must not** contain `Co-Authored-By` or any
  AI-attribution trailer (overrides the global Claude Code attribution reminder
  for this repo's commits). Push only to remotes under `github.com/pratham-projects`.
- Live reference app (read-only, parity source only, no data migration):
  `/private/tmp/claude-501/-Users-pratham-code-bunfiiliate/e65f5dcd-7117-44f9-ae63-f1a54fd6ae29/scratchpad/live`

## 1. Feature-gap matrix

Reference modules: `backend/src/modules/*` + `frontend/app/*` in the live app.
Status is against current `bunfiiliate` (crates/api, apps/web) post-T60.

| live module/page | status | decision |
| --- | --- | --- |
| auth (login/refresh/logout/me) | have | — |
| auth: affiliate `register` (workspace-scoped, no tenant creation — single-tenant app) | have (`affiliates/register` in Plan A) | — |
| **tenant/workspace self-serve signup** (does not exist in reference — single-tenant) | **missing** | build net-new — [B00](#b00-self-serve-signup-and-workspace-onboarding) |
| programs/sites/conversion-types/plans (CRUD) | have | — |
| affiliates/enrollments/codes | have | — |
| assignments (plan override, site assignment) | have via `enrollments.plan_override_id`; no per-affiliate *site* assignment | partial, skip — programs already scope affiliates via enrollment; per-affiliate site ACL not requested by any task, adding it is speculative scope |
| tracking (click/pixel beacon) | have (`tracker` crate) | — |
| events (`/webhooks`, `/conversions` inbound ingest) | have | — |
| webhooks: inbound Stripe/Paddle/generic | have (`Source::verify`, `inbound_events`) | — |
| webhooks: outbound endpoints + deliveries + redeliver | have | — |
| **webhooks: inbound test/replay tool** (`frontend/app/test/webhooks`, backend `/webhooks/history`, `/:id/retry`) | **missing** | build — [B01](#b01-inbound-webhook-tester) |
| conversions (list/approve/reject/reverse) | have | — |
| conversions: bulk-approve/bulk-reject | missing | skip — not named in any task below, adding is speculative scope; revisit only if a future task needs it |
| **chargebacks** (webhook-driven auto-reversal) | partial — admin can `reverse` manually; no automatic chargeback-webhook -> reversal wiring | build — [B02](#b02-chargeback-webhook-auto-reversal) |
| customers | have | — |
| ledger/payouts (admin-initiated batch: create/approve/paid/CSV) | have | — |
| **payouts: affiliate-initiated payout requests** (`payments`+`payout_requests` in reference — two overlapping modules there, one canonical feature) | **missing** | build — [B03](#b03-affiliate-payout-requests) |
| **payout rails beyond manual CSV** (PayPal Payouts, Wise, mock) | missing | build — [B04](#b04-payoutprovider-trait-and-rails) |
| **payout reconciliation** | missing | build — part of [B04](#b04-payoutprovider-trait-and-rails) |
| **tax/invoice basics** (W-8/W-9 capture, 1099-style export, per-payout invoice) | missing | build — [B05](#b05-tax-forms-and-invoices) |
| analytics/reports (timeseries, breakdown, CSV) | have, single currency per program (by design, anti-pattern #29) | — |
| **multi-currency aggregation + exchange rates** | missing | build — [B06](#b06-multi-currency-and-exchange-rates) |
| notifications (in-app) | have | — |
| **notification channels: Telegram/Discord webhook** | missing | build — [B07](#b07-notification-channels) |
| email templates + SMTP (optional, off by default) | have | — |
| contact requests | have, no `amount`/`currency`/typed category | partial — build minor extension inside [B08](#b08-contact-request-typing) |
| settings (generic k/v) | have | — |
| **platform billing** (plans, limits, metering, provider) | **missing — no reference exists; reference app is single-tenant with no subscription/billing table at all** | build net-new — [B09](#b09-billing-plans-and-limits-schema), [B10](#b10-billingprovider-trait-and-mock), [B11](#b11-billing-provider-integration)|
| audit log | have | — |
| GeoIP, auto-approve, hold period | have (T42, T17) | — |
| backfill (one-off legacy data migration script in reference) | n/a | skip — brief forbids data migration; nothing to backfill into a greenfield system |
| dashboard (admin + affiliate `/me`) | have | — |

## 2. New traits (names/signatures only — implement in B-tasks, not here)

All in `crates/core` (pure, no IO) as traits; adapters implementing them live in
`crates/api` behind a runtime-selected enum, mirroring the existing `Source`
pattern (`Source::verify`) — **enum of variants, not `dyn Trait` objects**, so it
stays consistent with `docs/PLAN.md` "Skipped on purpose: ... typestate generics".

```rust
// crates/core/src/billing.rs
trait BillingProvider {
    fn create_checkout(&self, plan: &BillingPlan, workspace: WorkspaceId) -> Result<CheckoutUrl, BillingError>;
    fn verify_webhook(&self, headers: &Headers, body: &[u8]) -> Result<BillingEvent, BillingError>;
    fn cancel_subscription(&self, sub_id: &str) -> Result<(), BillingError>;
}
// variants: Mock, LemonSqueezy (chosen rail, see B10) — Polar/Paddle/Creem/PayPal/Lago/KillBill evaluated, not built

// crates/core/src/payout_provider.rs
trait PayoutProvider {
    fn payout(&self, dest: &PayoutDestination, amount: Money) -> Result<PayoutReceipt, PayoutError>;
    fn status(&self, receipt: &PayoutReceipt) -> Result<PayoutRailStatus, PayoutError>;
}
// variants: ManualCsv (have), PayPalPayouts, Wise, Mock

// crates/core/src/exchange_rate.rs
trait ExchangeRateSource {
    fn rate(&self, from: Currency, to: Currency, at: Date) -> Result<Rate, FxError>;
}
// variants: Frankfurter (frankfurter.app, free, no key), CachedFallback (last-known DB row), Mock

// crates/core/src/notify_channel.rs
trait NotifyChannel {
    fn send(&self, event: &NotificationEvent) -> Result<(), NotifyError>;
}
// variants: InApp (have), SignedWebhook (have, generalized), TelegramWebhook, DiscordWebhook, Smtp (have)
```

## 3. Task DAG

Branch prefix `feat/b-*`. Depends-on refers to B-task IDs or Plan A T-task IDs
(all done). Every task's acceptance command must pass under `make ci` additions
noted per task. Workers: `gemini | kiro | opencode`.

---

### B00: self-serve signup and workspace onboarding

- **branch**: `feat/b-signup-onboarding` · **worker**: kiro · **depends-on**: none (T20 auth done)
- **files**: `crates/api/src/workspaces.rs` (new public route), `crates/api/src/bootstrap.rs` (reuse `slugify`, `MIN_PASSWORD_LEN`), `crates/db/migrations/0051_signup.sql`, `apps/web/src/app/signup/` (new page), `sdks/ts/openapi.json` (regenerate)
- **schema sketch** (migration 0051):
  ```sql
  ALTER TABLE workspaces ADD COLUMN created_via text NOT NULL DEFAULT 'bootstrap'
    CHECK (created_via IN ('bootstrap','self_serve'));
  -- no other schema change: workspaces/users tables already support this shape
  ```
- **API**: `POST /api/v1/signup` (public, no auth) — body `{workspace_name, admin_email, admin_password}` ->
  creates `workspaces` row (`created_via='self_serve'`, slug from `slugify(workspace_name)`,
  de-duped with numeric suffix on collision), one `users` row (`role=admin`,
  `workspace_id` set, argon2 hash via existing `hash_password_bounded`), then
  logs the new admin in exactly like `POST /auth/login` (sets `bf_at`/`bf_rt`
  cookies). Rate-limit same as `/auth/login` (reuse `crates/api/src/limit.rs`).
  Returns 201 with the workspace + user, same shape as `GET /auth/me`.
- **steps**:
  1. Add `created_via` column, migration + `sqlx::test` fixture.
  2. Add `signup` handler in `workspaces.rs`: validate email format + password
     length (`MIN_PASSWORD_LEN`), slugify + collision loop (max 10 tries, 409
     after), transaction creating workspace + user, issue tokens via the same
     helper `auth.rs` uses for login (do not duplicate JWT logic).
  3. Register route in `routes()` merge list (public, before any `Auth` layer).
  4. `apps/web/src/app/signup/page.tsx`: form (workspace name, admin email,
     password, confirm password), on success redirect to `/dashboard`.
  5. Add nav link from `/login` ("Create a workspace").
  6. Regenerate SDK (`make openapi-check`).
- **acceptance**: `cargo test -p api signup && pnpm -C apps/web build && pnpm -C apps/web typecheck`
- **tests**:
  - `signup_creates_workspace_and_logs_in` (integration, `#[sqlx::test]`): POST
    `/signup` -> 201, response has `workspace.id`, `user.role == "admin"`;
    follow-up `GET /auth/me` with returned cookies -> 200, same workspace.
  - `signup_rejects_short_password` -> 400, no workspace row created (`COUNT(*)`
    unchanged).
  - `signup_slugifies_and_dedupes` -> two signups with workspace name `"Acme"`
    produce slugs `acme` and `acme-2`.
  - `signup_rate_limited` -> Nth+1 rapid request from same IP -> 429 (reuse
    existing limiter test pattern from `auth.rs` tests).

---

### B01: inbound webhook tester

- **branch**: `feat/b-webhook-tester` · **worker**: gemini · **depends-on**: T16 (ingest-webhooks)
- **files**: `crates/api/src/webhooks.rs` (new routes), `apps/web/src/app/webhooks/` (extend existing page or add `test` tab)
- **no schema change** — reuses `inbound_events` (already has `program_id, source, external_id, payload, received_at, processed_at, error`).
- **API**:
  - `GET /api/v1/webhooks/inbound?program_id=&source=&limit=` — paged list from
    `inbound_events` (admin, workspace-scoped join through `programs`).
  - `POST /api/v1/webhooks/inbound/{id}/replay` — re-enqueues the stored
    `payload` through the same worker ingest path used on first receipt (do not
    re-verify signature; it already passed once to be stored).
  - `POST /api/v1/webhooks/test` — admin-only, body `{program_id, source: "generic"|"stripe"|"paddle", payload}`
    — server-side builds a correctly signed request using the program's own
    stored secret and posts it to the tracker's real inbound endpoint over
    loopback (`TRACKER_INTERNAL_URL`, reuse `reqwest` client already in
    `crates/api/Cargo.toml`), returning the tracker's raw response status + body
    so an admin can verify their own Stripe/Paddle secret end-to-end without a
    real provider account.
- **steps**:
  1. Add `list_inbound`, `replay_inbound`, `test_webhook` handlers.
  2. `test_webhook` signs exactly as `Source::verify` expects (reuse the HMAC
     helper from `tracker`'s webhook module — move it to `bf-core` if private,
     since both tracker and api need it; do not duplicate signing logic).
  3. Frontend: extend `apps/web/src/app/webhooks/page.tsx` with a "Test" tab —
     program select, source select, JSON payload textarea (prefilled fixture
     per source), "Send test" button showing the raw response.
- **acceptance**: `cargo test -p api webhook_tester`
- **tests**:
  - `test_webhook_generic_roundtrips`: POST `/webhooks/test` with a valid
    generic payload -> tracker returns 202 -> `inbound_events` gains one row
    with `processed_at` set after the worker tick.
  - `test_webhook_bad_program_404`: unknown `program_id` -> 404, no outbound
    HTTP call made (assert via a stub tracker base URL that panics if hit).
  - `replay_inbound_reprocesses_without_reverify`: seed an `inbound_events` row
    with `processed_at` set and `error` set (simulate original failure), call
    replay, assert `error` cleared and a `conversions` row now exists.

---

### B02: chargeback webhook auto-reversal

- **branch**: `feat/b-chargeback-autoreverse` · **worker**: opencode · **depends-on**: T17 (worker-ingest), F3 (money-worker fixes)
- **files**: `crates/api/src/worker.rs` (ingest processor), `crates/core/src/state.rs` (if a new `ConversionEvent` variant is needed)
- **no schema change** — `conversion_status` already has `reversed`; `ledger_entries` chargeback posting already documented in `docs/PLAN.md` section 1 ("Chargeback after approval: debit AffPayable, credit Expense").
- **behavior**: Stripe sends `charge.refunded`/`charge.dispute.created`, Paddle
  sends `payment_refunded` — these currently land in `inbound_events` as
  `source=stripe|paddle` rows but the worker's ingest processor only builds new
  conversions from *charge-succeeded*-shaped payloads. Add a payload-type branch:
  if the event type is a refund/dispute/chargeback for an `external_id` that
  matches an existing `conversions.external_id`, look up that conversion and run
  its `transition(Chargeback)` (already exists as a `ConversionEvent` per
  `docs/PLAN.md`: "Chargeback after approval: debit AffPayable, credit Expense
  (may go negative; clawback rolls into next payout)") instead of creating a
  new conversion. If the matched conversion is still `pending` (not yet
  approved), use the existing `reject` path, not chargeback.
- **steps**:
  1. In the ingest savepoint handler (anti-pattern #23: one bad event = one
     savepoint), detect refund/dispute payload shape per source (Stripe:
     `type` field; Paddle: `alert_name`/`event_type` field — confirm exact
     field names against `Source::verify`'s existing payload parsing, do not
     invent new ones).
  2. Look up conversion by `(program_id, external_id)`; if none, record
     `error = "chargeback for unknown conversion"` on the `inbound_events` row
     and move on (do not fail the batch — anti-pattern #23).
  3. If found and `approved`: call the existing chargeback transition, emit the
     ledger txn, outbox notification.
  4. If found and `pending`: reject instead.
  5. Shrink any open (`draft`/`approved`, not yet `paid`) payout batch/payout
     request containing that conversion, per anti-pattern #26 (never pay a
     negative balance) — this reuses logic already written for T18's
     refund/reversal-before-payout handling; call it, do not reimplement.
- **acceptance**: `cargo test -p api chargeback_webhook`
- **tests**:
  - `stripe_dispute_reverses_approved_conversion`: seed an approved conversion
    with Stripe `external_id`, feed a dispute-shaped inbound event through the
    worker tick, assert conversion status `reversed`, ledger legs debit
    AffPayable / credit Expense, affiliate balance decreased.
  - `chargeback_on_pending_conversion_rejects_not_reverses`: same but
    conversion still `pending` -> ends `rejected`, no chargeback ledger txn.
  - `chargeback_unknown_external_id_records_error_not_failure`: event for an
    `external_id` with no matching conversion -> `inbound_events.error` set,
    worker tick returns ok (no panic, no stuck row), batch continues
    processing other events in the same tick.
  - `chargeback_shrinks_open_payout`: conversion is part of an `approved`
    (unpaid) payout batch; after chargeback, batch's payable total decreases
    by that conversion's commission.

---

### B03: affiliate payout requests

- **branch**: `feat/b-payout-requests` · **worker**: kiro · **depends-on**: T18 (payouts-reversals)
- **files**: `crates/db/migrations/0052_payout_requests.sql`, `crates/api/src/payouts.rs` (extend), `apps/web/src/app/payouts/page.tsx` (admin side), new `apps/web/src/app/me/payouts` request button (affiliate side, extend existing T34 page)
- **schema sketch** (migration 0052):
  ```sql
  CREATE TYPE payout_request_status AS ENUM ('pending','approved','rejected','paid');

  CREATE TABLE payout_requests (
    id uuid PRIMARY KEY,
    affiliate_id uuid NOT NULL REFERENCES affiliates(id),
    program_id uuid NOT NULL REFERENCES programs(id),
    requested_cents bigint NOT NULL CHECK (requested_cents > 0),
    currency char(3) NOT NULL,
    status payout_request_status NOT NULL DEFAULT 'pending',
    rejection_reason text,
    payout_id uuid REFERENCES payouts(id), -- set once folded into a paid payout
    created_at timestamptz NOT NULL DEFAULT now(),
    decided_at timestamptz,
    decided_by uuid REFERENCES users(id)
  );
  CREATE INDEX ON payout_requests (affiliate_id, status);
  ```
  No `included_conversion_ids`/`excluded_conversion_ids` columns (that was the
  reference app's two-overlapping-modules design) — a request is just "pay me
  what I'm owed now"; sizing stays derived from the `aff_payable` ledger
  balance per anti-pattern #26, computed at approval time, not requested time
  (balance may move between request and decision).
- **API**:
  - `POST /api/v1/me/payout-requests` (affiliate) — body `{program_id,
    requested_cents, currency}`; server validates `requested_cents <=` current
    `aff_payable` balance for that program/affiliate and `>= programs`-level
    affiliate `payout_min_cents`; 409 if an existing `pending` request already
    exists for that program/affiliate (one at a time).
  - `GET /api/v1/me/payout-requests` (affiliate, own only), `GET
    /api/v1/payout-requests` (admin, workspace-scoped, filters
    `program_id`/`status`).
  - `POST /api/v1/payout-requests/{id}/approve` (admin) — re-checks the
    balance at approval time (anti-pattern #26), creates/attaches to a
    `payouts` row sized from the *current* balance (not the originally
    requested amount if the balance shrank from a chargeback since — cap at
    balance, never exceed it), marks `approved`.
  - `POST /api/v1/payout-requests/{id}/reject` (admin) — body
    `{reason}`.
- **steps**:
  1. Migration + repo functions in `bf-db`.
  2. Handlers in `payouts.rs` reusing the existing batch-creation/balance
     logic from T18 (do not duplicate the "size from aff_payable minus open
     payouts" function — extract it if not already a standalone fn).
  3. Web: affiliate `/me/payouts` gets a "Request payout" button (disabled
     below `payout_min_cents`); admin `/payouts` gets a "Requests" tab listing
     pending requests with approve/reject.
  4. Outbox notification to the admin on request creation, to the affiliate on
     decision (in-app, reuses existing `notifications` table).
- **acceptance**: `cargo test -p api payout_requests`
- **tests**:
  - `request_capped_at_payable_balance`: request for more than the affiliate's
    `aff_payable` -> 400.
  - `request_blocked_below_minimum`: request under `payout_min_cents` -> 400.
  - `duplicate_pending_request_conflicts`: second request while one is
    `pending` -> 409.
  - `approve_sizes_from_current_balance_not_requested_amount`: balance shrinks
    (simulate a chargeback) between request and approval -> approved payout
    amount equals the *current* balance, not the original request.
  - `reject_requires_reason_and_notifies`: reject without `reason` -> 400;
    with reason -> `notifications` row created for the affiliate.

---

### B04: PayoutProvider trait and rails

- **branch**: `feat/b-payout-providers` · **worker**: opencode · **depends-on**: B03
- **files**: `crates/core/src/payout_provider.rs` (new), `crates/api/src/payouts.rs` (wire provider selection), `crates/db/migrations/0053_payout_rail.sql`
- **schema sketch**:
  ```sql
  ALTER TABLE payouts ADD COLUMN rail text NOT NULL DEFAULT 'manual_csv'
    CHECK (rail IN ('manual_csv','paypal_payouts','wise','mock'));
  ALTER TABLE payouts ADD COLUMN rail_receipt jsonb; -- provider reference id, raw status
  ALTER TABLE payouts ADD COLUMN rail_status text; -- provider-reported status, nullable until dispatched
  ```
- **free-tier facts to honor**: PayPal Payouts API has a sandbox + a real free
  tier for low volume (percentage fee, no monthly cost) — use as the real
  rail. Wise has a developer sandbox (no production without a business
  agreement) — ship it behind the same trait but default-off / sandbox-only in
  tests; document the production gap in `docs/self-hosting.md` rather than
  pretending it's turnkey. `Mock` always available for tests and as the
  default when no provider credentials are configured (fail-safe: unset env =
  `manual_csv` stays the only *active* rail, others just can't be selected).
- **API**: extend `POST /api/v1/payouts/batches/{id}/mark-paid` with optional
  body `{rail: "manual_csv"|"paypal_payouts"|"wise"|"mock"}` (default
  `manual_csv`, unchanged behavior); when a non-manual rail is chosen, call
  `PayoutProvider::payout` per payout line instead of just flipping status,
  store `rail_receipt`/`rail_status`, and add `GET
  /api/v1/payouts/batches/{id}/reconcile` (admin) which calls
  `PayoutProvider::status` for every non-`paid`-confirmed-by-provider line and
  updates `rail_status` (does not change ledger `payout_status`, which stays
  the source of truth for money — this is a side-channel health check, not a
  second ledger).
- **steps**:
  1. Define trait + `Mock` impl (deterministic success) + `PayPalPayouts` impl
     (sandbox REST, API key/secret from env, off unless both set) + `Wise`
     impl (sandbox REST, same env-gating pattern).
  2. Env vars: `PAYPAL_PAYOUTS_CLIENT_ID`/`_SECRET`/`_SANDBOX`,
     `WISE_API_TOKEN`/`_SANDBOX` — document in `.env.example` and
     `docs/self-hosting.md`, both optional, unset = rail unavailable (422 if
     selected without config, not a panic).
  3. Wire selection into `mark_paid` handler; reconciliation handler.
  4. Web: batch sheet gets a rail selector (default Manual CSV) and shows
     `rail_status` next to each payout line after reconcile.
- **acceptance**: `cargo test -p api payout_rail`
- **tests**:
  - `mark_paid_manual_csv_unchanged`: existing T18 CSV-export behavior
    untouched when `rail` omitted (regression guard).
  - `mark_paid_mock_rail_stores_receipt`: `rail: "mock"` -> `rail_receipt` has
    a mock reference id, `rail_status = "success"`.
  - `mark_paid_unconfigured_rail_422`: `rail: "paypal_payouts"` with no env
    set -> 422, ledger untouched, no partial state.
  - `reconcile_updates_rail_status_without_touching_ledger`: call reconcile
    twice with a provider status transition stubbed via `Mock` -> `rail_status`
    changes, `payout_status` (ledger-governing) does not.

---

### B05: tax forms and invoices

- **branch**: `feat/b-tax-invoices` · **worker**: gemini · **depends-on**: T18, B03
- **files**: `crates/db/migrations/0054_tax_forms.sql`, `crates/api/src/affiliates.rs` (self-service form capture), `crates/api/src/payouts.rs` (invoice render), new `crates/api/src/invoice.rs`
- **schema sketch**:
  ```sql
  CREATE TYPE tax_form_kind AS ENUM ('w8', 'w9', 'none');

  ALTER TABLE affiliates ADD COLUMN tax_form_kind tax_form_kind NOT NULL DEFAULT 'none';
  ALTER TABLE affiliates ADD COLUMN tax_legal_name text;
  ALTER TABLE affiliates ADD COLUMN tax_country char(2);
  ALTER TABLE affiliates ADD COLUMN tax_id_last4 text; -- never store full SSN/EIN/TIN, last 4 only
  ALTER TABLE affiliates ADD COLUMN tax_form_submitted_at timestamptz;
  ```
  Deliberately **not** storing a full SSN/EIN — last-4 only, same spirit as
  anti-pattern #9 (no secrets committed) extended to PII: this is a
  self-hosted OSS product, not a KYC vendor; full TIN capture is explicitly
  out of scope (self-hosters who need real 1099 filing attach their own
  compliance vendor via the exported CSV).
- **API**:
  - `PUT /api/v1/me/tax-form` (affiliate) — body
    `{kind: "w8"|"w9", legal_name, country, tax_id_last4}`.
  - `GET /api/v1/reports/1099-style.csv` (admin) — one row per affiliate with
    total `paid` payouts in the calendar year >= a threshold (env
    `TAX_REPORT_THRESHOLD_CENTS`, default `60000` i.e. $600, the real US 1099-NEC
    threshold used only as a sane default, not a compliance claim — document
    in `docs/self-hosting.md` that this is not tax advice), columns
    `legal_name, country, tax_form_kind, total_paid_cents, currency`. Quote
    per RFC 4180, defuse leading `=+-@` (reuse `analytics::csv_cell` /
    `payouts::field`, anti-pattern #25 — do not write a third CSV escaper).
  - `GET /api/v1/payouts/{id}/invoice.html` (admin + the owning affiliate) —
    self-contained HTML (no JS, matches `site/index.html`'s zero-framework
    style), fields: workspace name, affiliate legal name (falls back to
    affiliate email if no tax form on file), payout id, paid date, line items
    (conversion id, amount, commission), total, currency.
- **steps**:
  1. Migration.
  2. Tax form self-service handler + validation (`country` is a 2-letter
     code, `tax_id_last4` is exactly 4 digits if present).
  3. 1099-style CSV report handler, reusing existing CSV escaping helper.
  4. Invoice HTML handler (server-rendered string template, no new templating
     crate — reuse `minijinja`, already a dependency via `mail.rs`).
  5. Web: affiliate `/me/profile` gets a tax-form section; admin payout batch
     sheet gets an "Invoice" link per payout line.
- **acceptance**: `cargo test -p api tax_invoices`
- **tests**:
  - `tax_form_rejects_non_4digit_id`: `tax_id_last4: "12"` -> 400.
  - `tax_form_rejects_full_id_looking_value`: 9+ digit value -> 400 (guard
    against accidental full-TIN submission).
  - `report_1099_csv_escapes_formula_injection`: affiliate legal name
    `=1+1` -> CSV cell is quoted/prefixed per the shared escaper (regression
    against anti-pattern #25, not a new rule).
  - `report_1099_csv_only_includes_above_threshold`: affiliate paid $500 total
    excluded at default threshold, $700 included.
  - `invoice_html_renders_line_items_matching_ledger`: invoice totals equal
    sum of the payout's conversion commissions (cross-check against
    `ledger_entries`, not against `payouts.amount_cents` alone, to catch drift).

---

### B06: multi-currency and exchange rates

- **branch**: `feat/b-multi-currency` · **worker**: opencode · **depends-on**: none (touches T23 analytics, additive)
- **files**: `crates/core/src/exchange_rate.rs` (new), `crates/db/migrations/0055_exchange_rates.sql`, `crates/api/src/analytics.rs` (extend "all programs" aggregation)
- **schema sketch**:
  ```sql
  CREATE TABLE exchange_rates (
    base char(3) NOT NULL,
    quote char(3) NOT NULL,
    rate numeric(18,8) NOT NULL,
    as_of date NOT NULL,
    fetched_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (base, quote, as_of)
  );
  -- cache table; worker tick upserts today's rate, stale rows kept for history/offline fallback
  ```
- **source**: `frankfurter.app` (free, no API key, ECB-sourced, documented
  stable). Fetched by a new worker tick (plain `tokio` interval per
  `docs/PLAN.md` section "Skipped on purpose: ... job-queue/cron crates"),
  once per day per currency pair actually in use (derived from distinct
  `programs.currency` values), upserted into `exchange_rates`. If the fetch
  fails (network down, rate-limited), **offline fallback** = reuse the most
  recent `as_of` row already cached; if none exists yet for a pair, the
  aggregation that needs it is simply omitted (same as anti-pattern #29's
  existing "show no money and ask for a program" behavior), never silently
  substitute 1:1.
- **API**: `GET /api/v1/analytics/summary-all?display_currency=USD` (admin,
  workspace-scoped, new endpoint — does not change the existing
  per-program `analytics/summary`) — sums every program's revenue/commission
  converted to `display_currency` using the latest cached rate `<=` the
  rollup's date; programs whose currency has no cached rate for that date are
  excluded from the sum and listed separately as `unconverted: [{program_id,
  currency, amount_cents}]` so totals are never silently wrong.
- **steps**:
  1. Migration.
  2. `ExchangeRateSource` trait + `Frankfurter` impl (reqwest GET,
     `https://api.frankfurter.app/{date}?from={base}&to={quote}`) + `Mock`.
  3. Worker tick: once/day, for each distinct `(base, quote)` pair actually
     needed (every program currency -> every *other* program currency present
     in the same workspace, not a full N×N of ISO currencies), fetch and
     upsert.
  4. `summary-all` handler: join rollups to latest-available rate per program
     currency, convert, sum, report `unconverted` list for anything stale/missing.
  5. Web: dashboard gets a "Display currency" selector (only shown when a
     workspace actually has >1 currency across its programs — hidden
     otherwise, no new UI for the common single-currency case).
- **acceptance**: `cargo test -p api multi_currency`
- **tests**:
  - `rate_tick_upserts_only_pairs_in_use`: two programs (`USD`, `EUR`) -> after
    tick, `exchange_rates` has rows for `USD->EUR` and/or `EUR->USD` (direction
    per implementation), none for unrelated pairs like `JPY->GBP`.
  - `summary_all_converts_using_latest_rate_leq_date`: seed rates for two
    dates, assert the rollup dated between them uses the earlier (`<=`) rate,
    never a future one.
  - `summary_all_excludes_and_lists_unconverted_when_rate_missing`: a third
    program's currency has no cached rate -> excluded from the summed total,
    present in `unconverted` with its own amount.
  - `fetch_failure_falls_back_to_last_cached_not_1to1`: stub `Frankfurter` to
    error -> tick keeps the previous day's row untouched, no `rate = 1.0`
    row ever written as a fallback.

---

### B07: notification channels

- **branch**: `feat/b-notify-channels` · **worker**: gemini · **depends-on**: T24 (ops-routes, outbound webhook delivery)
- **files**: `crates/api/src/ops.rs` (settings), `crates/api/src/dispatch.rs` (outbox dispatcher — add channel fan-out), `crates/db/migrations/0056_notify_channels.sql`
- **schema sketch**:
  ```sql
  ALTER TABLE workspaces ADD COLUMN notify_telegram_webhook text; -- bot webhook/chat URL, plaintext (same trust model as program secrets, anti-pattern #9 scope: no KMS)
  ALTER TABLE workspaces ADD COLUMN notify_discord_webhook text;
  ```
  No new table — these are workspace-level settings, same tier as existing
  `workspaces.settings jsonb`; added as real columns (not buried in jsonb)
  because they're validated URLs checked on every dispatch, matching how
  `programs.webhook_secret` is a real column, not jsonb.
- **behavior**: the outbox dispatcher (`dispatch.rs`) already fans out signed
  webhooks per `webhook_endpoints`; add two more static sinks per workspace
  when the corresponding URL column is set: POST a short plain-text summary
  (not the full signed payload shape — Telegram/Discord expect `{"text":...}`
  / `{"content":...}` respectively) for a configurable subset of event kinds
  (reuse the existing outbox `topic` field to filter: default = payout
  paid/rejected, chargeback, new payout request).
- **steps**:
  1. Migration.
  2. `NotifyChannel` trait + `TelegramWebhook`/`DiscordWebhook` impls (plain
     `reqwest::Client::post`, existing SSRF guard from `crates/api/src/ssrf.rs`
     applies — these are still outbound URLs an admin controls, same risk
     class as `webhook_endpoints.url`, reuse the guard, do not special-case it
     away).
  3. Wire into `dispatch.rs`'s existing outbox loop as additional sinks,
     same retry/backoff policy already written for signed webhooks (do not
     fork a second retry mechanism).
  4. Settings UI: two URL fields under `/settings`, "Send test message" button
     (`POST /api/v1/settings/test-notify-channel` body `{channel}`).
- **acceptance**: `cargo test -p api notify_channels`
- **tests**:
  - `telegram_webhook_sends_plain_text_on_payout_paid`: trigger a payout-paid
    outbox event with `notify_telegram_webhook` set -> one POST captured
    (test HTTP server) with `{"text": ...}` shape, payout id in the text.
  - `discord_webhook_respects_topic_filter`: an event kind not in the default
    filter set -> no POST sent.
  - `notify_channel_url_goes_through_ssrf_guard`: a private-IP URL set as
    `notify_discord_webhook` -> rejected at save time (400) unless
    `BF_ALLOW_PRIVATE_WEBHOOKS` is set, mirroring F2's existing webhook SSRF
    test pattern exactly.
  - `test_notify_channel_endpoint_sends_without_waiting_for_real_event`:
    `POST /settings/test-notify-channel` -> immediate POST to the configured
    URL, independent of the outbox.

---

### B08: contact request typing

- **branch**: `feat/b-contact-typing` · **worker**: kiro · **depends-on**: none (extends T24's `contact_requests`)
- **files**: `crates/db/migrations/0057_contact_request_type.sql`, `crates/api/src/ops.rs`
- **schema sketch**:
  ```sql
  CREATE TYPE contact_request_kind AS ENUM ('general_inquiry','technical_support','account_issue','payout_dispute');
  ALTER TABLE contact_requests ADD COLUMN kind contact_request_kind NOT NULL DEFAULT 'general_inquiry';
  ALTER TABLE contact_requests ADD COLUMN amount_cents bigint;
  ALTER TABLE contact_requests ADD COLUMN currency char(3);
  ALTER TABLE contact_requests ADD CONSTRAINT contact_amount_requires_currency
    CHECK ((amount_cents IS NULL) = (currency IS NULL));
  ```
- **API**: extend existing `POST /api/v1/contact-requests` to accept optional
  `kind` (default `general_inquiry`) and optional `amount_cents`+`currency`
  (both-or-neither, enforced by the check constraint above, mirrored in
  handler validation for a clean 400 instead of a DB constraint error); extend
  the admin list/filter with `?kind=`.
- **steps**: migration; extend existing request struct + validation; extend
  admin filter UI dropdown.
- **acceptance**: `cargo test -p api contact_requests`
- **tests**:
  - `contact_request_amount_requires_currency`: `amount_cents` set,
    `currency` omitted -> 400 (caught in handler, never reaches the DB
    constraint).
  - `contact_request_defaults_to_general_inquiry`: no `kind` in body -> stored
    row has `kind = 'general_inquiry'`.
  - `admin_filters_by_kind`: seed four requests one per kind, filter by
    `payout_dispute` -> exactly one returned.

---

### B09: billing plans and limits schema

- **branch**: `feat/b-billing-schema` · **worker**: opencode · **depends-on**: none
- **files**: `crates/db/migrations/0058_billing.sql`, `crates/db/src/billing.rs` (new repo module)
- **schema sketch**:
  ```sql
  CREATE TABLE billing_plans (
    id uuid PRIMARY KEY,
    key text UNIQUE NOT NULL,        -- 'free', 'pro', etc. — self-hosters can add rows
    name text NOT NULL,
    price_cents bigint NOT NULL,
    currency char(3) NOT NULL DEFAULT 'USD',
    max_programs int,                -- NULL = unlimited
    max_affiliates int,
    max_monthly_clicks bigint,
    is_default boolean NOT NULL DEFAULT false
  );
  CREATE UNIQUE INDEX ON billing_plans (is_default) WHERE is_default; -- at most one default

  CREATE TYPE subscription_status AS ENUM ('active','past_due','canceled');
  CREATE TABLE subscriptions (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    plan_id uuid NOT NULL REFERENCES billing_plans(id),
    provider text NOT NULL DEFAULT 'mock',
    provider_subscription_id text,
    status subscription_status NOT NULL DEFAULT 'active',
    current_period_end timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now()
  );
  ```
  Plans/limits as **DB rows**, per the brief, not hardcoded constants — a
  self-hoster who never touches billing gets one seeded `free` row
  (`is_default = true`, all limits `NULL` i.e. unlimited) and every workspace
  gets an implicit `subscriptions` row pointing at it via `bootstrap`/`signup`
  (B00), so metering code always has a plan to check against, no nullable
  "no plan" branch anywhere downstream.
- **steps**:
  1. Migration + seed the `free` default plan in the same migration
     (idempotent `INSERT ... ON CONFLICT DO NOTHING`).
  2. `bf-db::billing` repo: `plan_for_workspace`, `usage_for_workspace`
     (counts current `programs`, `affiliates`, trailing-30-day `clicks` —
     reuses existing tables, no new counters table).
  3. B00's signup handler gets one extra statement: insert the `subscriptions`
     row pointing at the default plan, in the same transaction as workspace
     creation (update B00's step 2 when both land; if B09 merges after B00,
     add a one-line migration backfilling `subscriptions` for any workspace
     missing one).
- **acceptance**: `cargo test -p bf-db billing`
- **tests**:
  - `every_workspace_has_a_default_subscription_after_backfill`: seed a
    workspace via the old path (no subscription row), run backfill, assert
    exactly one `subscriptions` row pointing at `is_default`.
  - `only_one_default_plan_allowed`: inserting a second `is_default=true` row
    -> unique-partial-index violation (constraint test, not application
    logic).
  - `usage_for_workspace_counts_match_live_tables`: seed 3 programs, 5
    affiliates -> `usage_for_workspace` returns `{programs: 3, affiliates: 5,
    ...}`.

---

### B10: BillingProvider trait and mock

- **branch**: `feat/b-billing-trait` · **worker**: gemini · **depends-on**: B09
- **files**: `crates/core/src/billing.rs` (new), `crates/api/src/billing.rs` (new routes)
- **evaluation (decision, not code)**: of LemonSqueezy / Paddle (billing, not
  the payment-event sense used elsewhere in this doc) / Polar / Creem /
  self-hosted Kill Bill / self-hosted Lago — **LemonSqueezy** is the chosen
  real rail: free to integrate (no monthly fee, percentage-only, acts as
  merchant of record so no separate tax/VAT handling needed — relevant since
  this plan explicitly avoids building real tax compliance in B05), simple
  webhook+checkout-link model that fits the existing outbox/webhook pattern
  with no new infra. Self-hosted Lago/Kill Bill rejected for this plan: both
  need their own always-on service, which contradicts "one compose stack,
  no new services" from `docs/PLAN.md`'s architecture. Paddle/Polar/Creem
  documented as acceptable future `BillingProvider` variants if a self-hoster
  prefers them — the trait is the extension point, only one real impl ships
  now.
- **API**:
  - `POST /api/v1/billing/checkout` (admin) — body `{plan_key}` -> returns a
    hosted checkout URL from the provider (or, for `Mock`, an internal URL
    that immediately activates the plan — lets `make e2e` cover this without
    a real LemonSqueezy account).
  - `POST /api/v1/webhooks/billing/:provider` (public, signature-verified like
    the existing Stripe/Paddle tracker webhooks) — on a subscription
    created/updated/canceled event, update `subscriptions`.
  - `GET /api/v1/billing/subscription` (admin) — current plan + usage vs
    limits (reuses B09's `usage_for_workspace`).
- **steps**:
  1. Trait + `Mock` + `LemonSqueezy` impl (reqwest, API key from env
     `LEMONSQUEEZY_API_KEY`, unset = provider unavailable, 422 on checkout
     attempt, same fail-safe pattern as B04).
  2. Webhook signature verification per LemonSqueezy's documented HMAC scheme
     (fixture-pinned, same approach as Stripe/Paddle fixtures in T16 — pin
     from official docs, don't guess).
  3. Metering: a lightweight `Auth` extractor wrapper (or a check inside
     mutating handlers that create programs/affiliates) consults
     `usage_for_workspace` + `plan_for_workspace`; over-limit creation -> 402
     Payment Required with a body naming which limit was hit.
- **acceptance**: `cargo test -p api billing`
- **tests**:
  - `checkout_mock_activates_plan_immediately`: `POST /billing/checkout` with
    `Mock` provider -> `subscriptions.plan_id` updated synchronously (no
    webhook round-trip needed in tests).
  - `checkout_unconfigured_provider_422`: `LemonSqueezy` selected, no API key
    -> 422, `subscriptions` untouched.
  - `billing_webhook_updates_subscription_status`: signed
    `subscription_cancelled`-shaped fixture -> `status = 'canceled'`.
  - `billing_webhook_bad_signature_400`: tampered body -> 400, no DB write
    (mirrors T16's `webhook` dup/bad-sig test pattern).
  - `over_plan_limit_create_program_returns_402`: workspace on a plan with
    `max_programs = 1`, already has 1 -> creating a second -> 402 naming
    `max_programs`.

---

### B11: billing provider integration polish

- **branch**: `feat/b-billing-ui` · **worker**: opencode · **depends-on**: B10
- **files**: `apps/web/src/app/settings/billing/` (new), `apps/web/src/app/admin` nav
- **scope**: admin-facing billing page — current plan, usage bars (programs/
  affiliates/clicks vs limits, hidden bars for `NULL`/unlimited), "Upgrade"
  buttons per available `billing_plans` row (hits B10's checkout endpoint),
  cancel button (confirm dialog, calls provider cancel then local status
  update). No new backend routes beyond B10.
- **acceptance**: `pnpm -C apps/web build && pnpm -C apps/web typecheck && pnpm -C apps/web lint`
- **tests**: browser-verified manually (per `CLAUDE.md`'s UI-change rule) —
  worker must start the dev stack, seed two plans, click through
  upgrade/cancel against the `Mock` provider, and report pass/fail in the
  brain log entry; no automated e2e test required for this task specifically
  (B10 already covers the API logic under test).

---

## 4. Risks and cuts

1. **Billing/payout rails have no production-grade sandbox parity.** PayPal
   Payouts and Wise both gate real production access behind manual business
   approval; LemonSqueezy's sandbox is the most honest of the three. Ship all
   three behind traits with `Mock` as the default-safe no-op, document the
   production gap plainly in `docs/self-hosting.md`, never claim turnkey.
2. **Tax/invoice scope is deliberately shallow.** No full TIN storage, no real
   e-filing, no VAT. This is a self-hosted OSS tool, not a compliance vendor —
   B05's CSV export is a starting point for a self-hoster's own accountant/
   vendor, say so in the doc it ships with.
3. **Multi-currency is additive, not a redesign.** `programs.currency` stays
   one currency per program (existing anti-pattern #29 decision untouched);
   B06 only adds a *display* aggregation layer on top, so nothing about
   existing single-currency correctness changes.
4. **Exchange-rate source is a single free API with no SLA.** Frankfurter has
   no published uptime guarantee; the offline-fallback-to-last-cached rule in
   B06 is the only resilience, by design (never fabricate a 1:1 rate).
5. **Cut from this plan**: bulk conversion approve/reject (reference has it,
   nothing here needs it — speculative scope); per-affiliate site ACLs beyond
   program enrollment (same reason); full KYC/TIN capture; any self-hosted
   billing engine (Lago/Kill Bill) requiring its own service process; backfill
   tooling (brief forbids data migration from the reference app).

## 5. Execution notes for implementation workers

- Every task above that touches money must keep anti-patterns #1, #2, #3,
  #11, #12, #26 intact — read `brain/wisdom/anti-patterns.md` before touching
  `ledger_entries`, `payouts`, or `payout_requests`.
- Every new outbound HTTP call (B01's test-webhook sender, B04's payout
  rails, B06's rate fetch, B07's notify channels, B10's checkout/LemonSqueezy
  calls) must go through the existing SSRF guard (`crates/api/src/ssrf.rs`)
  wherever the destination is admin-supplied (not for fixed first-party API
  hosts like `frankfurter.app` or `api.lemonsqueezy.com`, which are hardcoded,
  not user input).
- Every new migration continues the existing numbering from `0050_money_fixes.sql`
  (`0051`...`0058` above) — confirm the latest number in
  `crates/db/migrations/` before branching in case another B-task's migration
  landed first; renumber on conflict, never two migrations with the same
  number.
- Regenerate `sdks/ts/openapi.json` (`make openapi-check`) on every task that
  adds or changes a route.
