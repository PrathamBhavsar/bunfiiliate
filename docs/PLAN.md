# bunfiiliate: architecture and delivery plan

Rule: smallest complete change. No speculative abstraction. Clean-room product (no code, schema text or copy from any closed-source project).

Env notes: macOS arm64, Rust via brew rustup (prepend `/opt/homebrew/opt/rustup/bin` to PATH), Postgres 16 via brew (`/opt/homebrew/opt/postgresql@16/bin`), no docker locally. CI pins PG16: generate UUIDv7 in Rust (`uuid` feature `v7`), never use PG18 `uuidv7()`.

## 1. Architecture

```
bunfiiliate/
  Cargo.toml rust-toolchain.toml deny.toml Makefile .env.example
  crates/
    core/    bf-core   pure domain, no IO (optional `sqlx` feature only for enum derives)
    db/      bf-db     SQLx pool, migrations/, repos, outbox, advisory locks
    spool/   bf-spool  segmented WAL: [len u32|crc32|json] records, group-commit fsync, replay, checkpoint
    rt/      bf-rt     config (env), tracing, prometheus exporter, graceful shutdown
    tracker/ bin `tracker`  (+ js/tracker.src.js -> tracker.min.js via include_bytes!)
    api/     bin `api`: serve [--with-worker] | worker | openapi | bootstrap | seed-demo
  apps/web/       Next.js static export (output:'export'), Tailwind, shadcn, TanStack Query
  sdks/ts/        openapi.json + openapi-typescript types + openapi-fetch client
  bench/          oha scripts, results/*.json
  deploy/         Dockerfile (one image, both bins), compose.yml, Caddyfile, systemd/
  site/index.html standalone release page
  docs/           architecture.md ledger.md self-hosting.md benchmarks.md
  .github/workflows/ci.yml
```

Dependency direction: `core <- db <- {tracker, api}`, `spool <- tracker`, `rt <- {tracker, api}`. `core` does no IO, so money and state logic are proptest-able without a DB.

### Tracker hot path (no DB call per request)
1. Request validated against an in-memory `Snapshot` in an `ArcSwap` (code -> program/affiliate/landing; program key -> secrets). Loaded at startup, reloaded on Postgres `LISTEN bf_config`.
2. Tracker creates event id (UUIDv7), normalizes UA, referrer, country.
3. Event goes to the spool writer; handler awaits a oneshot ack. Writer drains queue, writes, fsyncs once per group (group commit), then acks.
4. Acked batches go over a bounded mpsc to the DB batch writer: `INSERT ... SELECT UNNEST(...) ON CONFLICT DO NOTHING`, then advance the checkpoint file. Segments below checkpoint are deleted.
5. Crash: replay from checkpoint. Idempotency keys make it safe (at-least-once + unique constraints = effectively-once).
6. Channel full: return 503 (load shedding), never unbounded memory.

fsync mode `always|off`, both benchmarked. Stripe/Paddle/generic conversions are only verified and spooled into `inbound_events` by the tracker; all business logic runs in the worker.

### Worker
A mode of `api` (`api worker`, or embedded `api serve --with-worker`), not a third binary. Loops claim jobs with `FOR UPDATE SKIP LOCKED`; periodic ticks take `pg_try_advisory_lock`. Plain `tokio` intervals: no job-queue or cron crate.
- Ingest processor: inbound event -> customer -> attribution -> fraud check -> commission -> conversion + ledger txn + outbox row, one DB transaction.
- Outbox dispatcher: signed webhooks (HMAC-SHA256, exponential backoff, max attempts, then dead), emails via lettre + minijinja, notifications.
- Ticks: auto-approve after hold period; daily rollups (idempotent upsert of yesterday and today).

### Auth
Caddy serves UI and `/api` same-origin (no CORS). Access JWT (15 min) in httpOnly `SameSite=Strict` cookie `bf_at`. Refresh token opaque httpOnly cookie `bf_rt` (path `/api/v1/auth`), rotates every use; reuse of an old token revokes the whole family. Origin check on every mutation. argon2 passwords. `Auth<R: Role>` extractor + workspace scope on every query.

### Key types (names only)
- `Money{cents:i64, currency}`, `Bps`, ID newtypes.
- `Account` (`Expense | AffPending | AffPayable | Cash`), `Posting`, `LedgerTxn::new() -> Result<_, Unbalanced>` (balanced by construction).
- `ConversionStatus` + `ConversionEvent` with `transition(self, ev) -> Result<(Status, LedgerTxn), InvalidTransition>`; `PayoutStatus`. Enum + transition table, not typestate generics (entities load from DB).
- `CommissionPlan::commission_for(amount, months_since_attribution)`.
- `trait Attribution`: `FirstTouch`, `LastTouch`.
- `trait FraudRule -> Verdict`: `IpVelocity` (governor), `BotUa`, `Dedupe` (governor keyed by visitor+code).
- `Source{Generic, Stripe, Paddle}::verify(headers, body, secret)`: enum, constant-time compare, no Stripe SDK.
- `NormalizedUa` (woothee), `SpoolRecord`, `Snapshot`.

### Ledger postings
- Conversion: debit Expense, credit AffPending.
- Approve: debit AffPending, credit AffPayable.
- Reject/refund before approval: debit AffPending, credit Expense.
- Chargeback after approval: debit AffPayable, credit Expense (may go negative; clawback rolls into next payout).
- Payout paid: debit AffPayable, credit Cash.

Commission = integer bps math, round-half-up, documented in `docs/ledger.md`.

## 2. Postgres schema outline

Enums: `user_role(super_admin, admin, affiliate)`, `affiliate_status(pending, active, suspended)`, `commission_kind(percent, flat)`, `commission_duration(one_month, x_months, lifetime)`, `attribution_model(first_touch, last_touch)`, `conversion_status(pending, approved, rejected, reversed, paid)`, `payout_status(draft, approved, paid, cancelled)`, `event_source(generic, stripe, paddle)`, `ledger_account(expense, aff_pending, aff_payable, cash)`, `rollup_dim(total, country, os, browser, device, referrer)`.

Tables:
- `workspaces(id, name, slug, settings jsonb)` (settings live here; no settings table)
- `users(id, workspace_id null, email citext uniq, password_hash, role)`
- `refresh_tokens(id, user_id, family_id, token_hash, expires_at, revoked_at, replaced_by)`
- `programs(id, workspace_id, name, currency char(3), attribution_model, cookie_days, auto_approve, hold_days, default_plan_id, public_key uniq, webhook_secret, stripe_secret, paddle_secret)`
- `program_sites(program_id, domain)` (with enrollments = multi-site assignment)
- `conversion_types(id, program_id, key, name)`
- `commission_plans(id, program_id, conversion_type_id null, kind, rate_bps, amount_cents, duration, months)`
- `affiliates(id, workspace_id, user_id, status, payout_email, payout_min_cents)`
- `enrollments(affiliate_id, program_id, plan_override_id null)`: plan resolution = affiliate override, then conversion-type plan, then program default
- `referral_codes(code pk, affiliate_id, program_id, landing_url)`
- `clicks(id uuidv7, program_id, affiliate_id, code, visitor_id, ts, ip_hash, country, device, os, browser, referrer_host, landing_path, fraud_reason null)`, BRIN on `ts`, no partitioning yet
- `inbound_events(id, program_id, source, external_id, payload jsonb, received_at, processed_at, error)`, unique `(program_id, source, external_id)`
- `customers(id, program_id, external_id, affiliate_id, first_click_id, attributed_at)`, unique `(program_id, external_id)`
- `conversions(id, program_id, customer_id, affiliate_id, conversion_type_id, external_id uniq/program, amount_cents, commission_cents, currency, status, occurred_at, approved_at, reversed_at)`
- `ledger_entries(id, txn_id, account, program_id, affiliate_id null, amount_cents signed, currency, conversion_id null, payout_id null, created_at)`: trigger rejects UPDATE/DELETE (append-only); DEFERRABLE constraint trigger enforces `sum(amount)=0` per `txn_id`; view `affiliate_balances` derives balances
- `payout_batches(id, program_id, status, created_by, approved_by, paid_at)`, `payouts(id, batch_id, affiliate_id, amount_cents, status, ledger_txn_id)`
- `outbox(id bigserial, topic, payload jsonb, available_at, attempts, locked_until, done_at)`
- `webhook_endpoints(id, workspace_id, url, secret, events text[], active)`, `webhook_deliveries(id, endpoint_id, outbox_id, attempt, status_code, next_attempt_at)`
- `daily_rollups(program_id, affiliate_id, day, dim, dim_value, clicks, conversions, revenue_cents, commission_cents)`, PK on all dimension columns
- `notifications(id, user_id, kind, body, read_at)`, `email_templates(id, workspace_id, key, subject, body)`, `contact_requests(id, workspace_id, program_id, name, email, message, status)`, `audit_log(id, workspace_id, actor_id, action, entity, entity_id, diff jsonb, at)`

## 3. API surface

Tracker (port 8081, metrics 9101 internal):
- `GET /t.js` (<2KB min, ETag, long cache)
- `GET /r/:code` records click, 302 to landing with `?bf_click=`; tracker.js sets first-party `bf_vid`, `bf_ref`
- `POST /c` click beacon `{code, vid, url, ref}` -> 204
- `POST /v1/events/:program_key` generic conversion; `X-Bf-Signature` HMAC + `Idempotency-Key`; 202
- `POST /v1/webhooks/stripe/:program_key`, `/v1/webhooks/paddle/:program_key`: signature verified (Stripe `t=,v1=` 5-min tolerance; Paddle `ts=;h1=`), 200 after spool fsync
- `GET /healthz`, `/readyz`

API (`/api/v1`, OpenAPI at `/api/openapi.json`, metrics 9100 internal) route groups: `auth` (login, refresh, logout, me), `workspaces` (super_admin), `members`, `programs` (sites, conversion-types, plans, secrets/rotate), `affiliates` (enrollments, override, codes), `customers`, `conversions` (list, approve, reject, reverse, bulk), `ledger` (entries, balances), `payouts` (batches: create from payable, approve, mark-paid, `export.csv`), `analytics` (timeseries, breakdown), `reports` (CSV), `notifications`, `contact-requests` (public POST, admin list), `settings`, `email-templates`, `webhooks` (endpoints, deliveries, redeliver), `audit-log`, `me/*` (affiliate portal: stats, links, earnings, payouts).

## 4. Task DAG

Every `feat/*` branch merges into `staging` with `--no-ff` once its acceptance check passes. `staging` merges into `prod` only after T60; push happens only then. Commit style: `feat: add ledger crate` (type: short what).

`make ci` = `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `cargo deny check`, web build, OpenAPI drift check, tracker.js size check.

| id | branch | owner | inputs | acceptance | deps |
|---|---|---|---|---|---|
| T00 | feat/workspace-scaffold | codex | tree above, empty crates, bf-rt tracing, Makefile, toolchain, deny.toml | `cargo clippy --workspace -- -D warnings && cargo test --workspace` | - |
| T01 | feat/ci | agy | ci.yml: fmt/clippy/test (pg16 service)/deny/web/openapi drift/compose config | `actionlint && cargo deny check` | T00 |
| T10 | feat/core-money-ledger | codex | Money, Bps, Account, Posting, LedgerTxn, commission math, proptest (balanced, no overflow, rounding bounds) | `cargo test -p bf-core` | T00 |
| T11 | feat/core-state-machines | codex | conversion/payout transitions emit LedgerTxn; proptest: any valid event sequence keeps ledger balanced and payable+pending = live commissions minus paid | `cargo test -p bf-core state` | T10 |
| T12 | feat/db-schema | codex | migrations (section 2), append-only + balance triggers, ledger repo, balances view, `#[sqlx::test]` | `sqlx migrate run && cargo test -p bf-db` | T00 |
| T13 | feat/spool | codex | segments, CRC framing, group commit, checkpoint, replay; torn-tail test | `cargo test -p bf-spool` | T00 |
| T14 | feat/tracker-hot-path | codex | axum app, Snapshot+LISTEN, /r /c /t.js, batch writer, woothee/referrer/CF-IPCountry normalization, 503 shedding | `cargo test -p tracker` (1000 clicks, kill, restart, exactly 1000 rows) | T12,T13 |
| T15 | feat/tracker-js | sonnet | tracker.src.js (cookie, beacon, bf_click), esbuild minify | `node scripts/size-check.mjs` (min < 2048B) | T00 |
| T16 | feat/ingest-webhooks | codex | `Source::verify` + fixtures from Stripe/Paddle docs, 3 tracker endpoints into inbound_events | `cargo test -p tracker webhook` (dup = 1 row, bad sig = 400) | T14 |
| T17 | feat/worker-ingest | codex | `api worker`, ingest processor, Attribution first/last + recurring by customer, plan resolution + duration window, outbox, auto-approve tick | `cargo test -p api flow_click_to_approved` | T11,T12,T16 |
| T18 | feat/payouts-reversals | codex | batch from payable >= min, approve, paid, CSV; refund/chargeback before and after payout | `cargo test -p api payouts` | T17 |
| T20 | feat/api-auth | codex | `api serve`, utoipa, error type, auth+rotation+reuse detection, roles, Origin check, audit helper, `bootstrap`, `seed-demo` | `cargo test -p api auth` | T12 |
| T21 | feat/api-programs-affiliates | codex | workspaces, members, programs, sites, types, plans, affiliates, enrollments, codes (+NOTIFY) | `cargo test -p api programs` | T20 |
| T22 | feat/api-money-routes | codex | customers, conversion actions, ledger, payout routes + CSV | `cargo test -p api money_routes` | T18,T21 |
| T23 | feat/analytics-rollups | codex | rollup tick, analytics + report CSV routes | `cargo test -p api analytics` | T17,T21 |
| T24 | feat/ops-routes | codex | outbound signed webhooks + retry, lettre emails + templates, notifications, contact requests, settings, audit list | `cargo test -p api ops` | T17,T21 |
| T25 | feat/ts-sdk | sonnet | `api openapi > sdks/ts/openapi.json`, types + client, drift script | `pnpm -C sdks/ts build && make openapi-check` | T22,T23,T24 |
| T30 | feat/web-shell | sonnet | static export, shadcn kit, login + refresh flow, role nav, DataTable/Form primitives; routes use `?id=` not `[id]` | `pnpm -C apps/web build && pnpm -C apps/web typecheck && pnpm -C apps/web lint` | T25 |
| T31 | feat/web-admin-core | sonnet | programs, sites, plans, affiliates, codes | same as T30 | T30 |
| T32 | feat/web-money | sonnet | conversions, customers, ledger, payouts (approve/CSV) | same as T30 | T30 |
| T33 | feat/web-analytics-ops | sonnet | analytics charts, reports, webhooks, templates, settings, audit, notifications, contacts | same as T30 | T30 |
| T34 | feat/web-affiliate-portal | sonnet | `/me` dashboard, links, earnings, payouts | same as T30 | T30 |
| T40 | feat/fraud-rules | codex | FraudRule trait, IpVelocity/BotUa/Dedupe; tracker flags clicks, worker holds conversions on flagged clicks | `cargo test fraud` | T14,T17 |
| T41 | feat/observability | codex | metrics: ingest latency histogram, spool lag/bytes, batch size, outbox depth, webhook failures | `/metrics` contains `bf_spool_lag` (in test) | T14,T17 |
| T42 | feat/geoip | codex | optional DB-IP lite mmdb (`maxminddb`), CF-IPCountry header first, `scripts/fetch-dbip.sh` | `cargo test -p tracker geo` | T14 |
| T43 | feat/load-test | agy | `bench/run.sh` (oha on /c, /r, webhook; fsync always vs off), `docs/benchmarks.md` with hardware, rps, p50/p99 | `bench/run.sh` exits 0, writes results/*.json | T40,T41 |
| T44 | feat/deploy | agy | Dockerfile, compose (pg, tracker, api --with-worker, caddy+web out), systemd units, self-hosting.md | `caddy validate --config deploy/Caddyfile` | T25,T30 |
| T50 | feat/oss-docs | agy | README, CONTRIBUTING, SECURITY, architecture.md (mermaid), ledger.md | `npx markdownlint-cli2 "**/*.md" "#node_modules"` | T44 |
| T51 | feat/release-page | agy | `site/index.html` (direction below) | `npx html-validate site/index.html && scripts/check-bench-numbers.sh` | T43 |
| T60 | feat/final-gate | agy | `make ci`; `ocr` (@alibaba-group/open-code-review) on `prod..staging`; `npx e2e` on seeded local stack (login -> program -> link -> click -> Stripe webhook -> approve -> payout CSV); findings return to owners as new feat/* tasks | `make ci` 0, both tools 0, reports in gitignored `.reports/`; then merge staging -> prod and push | all |

Check ocr and e2e README flags and any LLM key needs at the start of T60.

Parallel: T10, T12, T13, T15 right after T00. T20, T21 alongside T14-T18. T31-T34 in parallel. Demo checkpoint after T18: `api seed-demo` + curl drives the showpiece flow.

## 5. Release page direction
"Engineering datasheet": warm paper `#F4F1EA`, ink `#111`, hairline rules, one safety-orange accent `#FF5A1F` used only for measured numbers. IBM Plex Mono tabular figures for data, one grotesk for headings. Architecture diagram as inline SVG schematic: tracker -> spool -> batch writer -> PG -> worker -> outbox. Benchmark table straight from `bench/results`. One self-contained HTML file, no JS framework, no gradients, no emoji.

## 6. Risks and cuts
1. Spool/batch-writer correctness: per-record CRC, torn-tail replay test, kill-restart test, DB unique keys as backstop. Cut: segment rotation (one file). Never cut idempotency keys.
2. Commission semantics: pure functions in `core` + proptest + golden table. Cut: per-conversion-type plans.
3. Static-export limits: `?id=` routes, client guard, API is the auth boundary. No BFF.
4. CRUD/UI breadth: generated SDK + shared DataTable/Form. Cut order: contact requests, email-template editor (ship file defaults), notifications UI, rollup dims beyond country/device.
5. ocr/e2e non-deterministic, may need API keys: time-box, advisory; our own tests are the gate. Stripe/Paddle formats: pin fixtures from official docs. Cut: Paddle.

Skipped on purpose: Kafka/Redis/NATS, BFF, GraphQL, ORM, typestate generics on DB entities, FX (one currency per program), KMS (secrets in plaintext columns, trusted VPS DB), table partitioning, SSO/2FA/i18n, k6, testcontainers (`#[sqlx::test]` on local Postgres), Kubernetes/Helm, separate worker binary, job-queue/cron crates, Code of Conduct and issue templates in v1.
