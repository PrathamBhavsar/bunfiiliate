# Plan v2 revisions (binding; overrides A/B/C where they differ)

From the three goldfish reviews (tribe #4, #5, #6). Workers read this file first.

## Global

- **Migrations.** `0060` is taken (secrets). Numbers are fixed: A05=0061, A06=0062, A07=0063, A12=0064, A08 token_version=0065 (own file; A06/A07 depend on A08). B: B00=0070, B02=0071, B03=0072, B04=0073, B05=0074, B06=0075, B07=0076, B09=0077, B10=0078, B11 or other=0079. C: C20=0080, C05 spike=0081 (flag only). Section numbers in the specs are superseded.
- **Secrets.** Every new credential column (payout provider creds, Telegram/Discord URLs, billing provider secret) uses `Vault::seal_col`, is added to `api::secrets::PROTECTED` (and its `*_enc_at` marker), is redacted on GET, and has a leak test. See `crates/api/src/secrets.rs`.
- **Traits.** Types and pure logic in `bf-core`; HTTP adapters live in `api` and are `async`. No IO in core.
- **Commits.** No Co-Authored-By or tool attribution on worker commits (plan rule). Pushing is out of scope until the final gate; origin is `github.com/PrathamBhavsar/bunfiiliate`.
- **Gate per task.** `make ci` (includes cargo deny, openapi-check). Brain log + tasks updated by the coordinator at merge.
- Workers use their own git worktree (`git worktree add ../wt-<id> -b <branch> staging`), commit on their branch, never merge, never touch staging/prod. Only the coordinator runs `make e2e` (fixed ports).

## Plan A

- A05: recovery codes stored as salted argon2, never SHA-256. TOTP `last_used_step` update is atomic: `UPDATE user_totp SET last_used_step=$new WHERE user_id=$1 AND (last_used_step IS NULL OR last_used_step < $new) RETURNING 1`; no row = replay.
- A08: token_version in `0065_token_version.sql`. Tokens without `ver` count as version 0 (the column default), so existing sessions survive the upgrade until a reset. The per-instance version cache TTL is 5 s, documented as the revocation bound across instances. A06/A07 depend on A08.
- A09: CSRF applies only to cookie-authenticated requests; requests authenticated by `Authorization` bearer or API key are exempt. A06 depends on A09 (csrf endpoint).
- A06 splits: **A06a** reset core (route, TOTP/recovery verify, session revoke; validate the new password BEFORE consuming a code; unknown email does equal dummy work). **A06b** throttles: reset throttle per user id; login throttle keyed by `(lower(email), ip)` with backoff delay, never a hard per-email lockout (no lockout DoS). Min-12 password applies to new passwords (register, reset, change) only, not to login.
- A07: Notifier is in-app + optional SMTP only; drop resend/brevo. Admin-assisted reset is an explicit recovery path that works for users with TOTP enrolled; it is audited and sends an in-app notification to the user.
- A10 also depends on A07 and A12. Dev `totp-code` CLI sits behind cargo feature `dev-tools`, off in release.
- A12 splits: **A12a** routes + strict schema validation (`deny_unknown_fields` on v2 only) + signature (`<t>.<key>.<body>`, idempotency key charset `[A-Za-z0-9_-]{1,64}`). **A12b** idempotency (duplicate returns the first event's id, looked up by external id) + outbound webhook `api_version`.
- Outbound webhook v2 body is opt-in per endpoint: `webhook_endpoints.api_version` text default `v1`; v1 body unchanged.
- A16 gate includes `ocr`.

## Plan B

- Order: B09 (billing schema, owns the subscription insert) before B00. B00 signup is gated by `SIGNUP_ENABLED` (default off); no email verification (out of scope); role `admin` = workspace admin, bootstrap stays `super_admin`. B00 needs a cross-workspace isolation test.
- B01/B02: the worker reads `crates/tracker/src/webhook.rs` first and records, in the task result, the exact Stripe/Paddle event type strings and JSON path used to find the original charge, with fixtures. Target types: Stripe `charge.refunded`, `charge.dispute.created`; Paddle `adjustment.created` (refund, chargeback). Matching must work when the refund event id differs from the sale's event id; test it. `TRACKER_INTERNAL_URL` is new: env, default `http://127.0.0.1:8081`, compose `http://tracker:8081`.
- B03: partial unique index `(affiliate_id, program_id) WHERE status='pending'` plus a concurrent-request test; approve locks the balance with `SELECT ... FOR UPDATE`; test currency mismatch vs the program.
- B04: no external call inside a DB transaction. Payout lines go pending -> submitted -> paid|failed; provider idempotency key = payout line id; the ledger posts only on confirmed paid. Test provider failure mid-batch and retry without double pay. Provider credentials sealed.
- B05: plain escape function (HTML-escape legal_name/email); no minijinja.
- B06: rates `numeric(18,8)`; conversion to integer cents rounds half-even; cross rates via USD triangulation; verify the Frankfurter host at implementation time.
- B07: Telegram/Discord URLs sealed and redacted. SMTP only for mail.
- B10: dedupe provider events by event id, order by `updated_at`, map workspace via `custom_data.workspace_id`, provider route allowlist `mock|lemonsqueezy`. A racy plan-limit check is acceptable; note it.
- B11 needs an automated gate: `pnpm -C apps/web typecheck build lint` plus an e2e case.

## Plan C

- C20: rename the old table's indexes before creating new ones (index names do not follow `RENAME TABLE`); partition bounds explicit UTC; move rows out of `clicks_default` before attach; copy in batches, not one long transaction. The click insert uses bare `ON CONFLICT DO NOTHING`.
- C02/C04: C02 ships with `crates/api/tests/known_leaks.txt` (xfail list); C04 empties it. C02 includes role-escalation cases (affiliate on admin routes, unauthenticated, wrong role). C01 annotation kinds include API-key and inbound-webhook routes. C16 depends on C01.
- C03: state in the lint doc that it cannot see `format!`-built, multi-line, or `query!` SQL; ship a failing fixture for each.
- C10: pause spool GC (flag file), copy spool segments, THEN `pg_dump` (DB ahead of checkpoint is safe; replay is idempotent), resume GC.
- C11 per route when the spool disk is full: `/c` 503; `/r/{code}` still redirects but records no click (called out in docs); `/v1/events` and webhook routes 503.
- C12: alerts go to super admins only; `BF_ALERT_TOKEN` unset disables the route; token compare constant-time; `bf_clicks_default_rows` is defined in C20, not C12.
- C16: the API has no healthz/readyz today; add them.
- C21 depends on C30. C22: rollups are never recomputed for days whose partitions were dropped; retention must be >= the rollup window.
- C90: hygiene grep is for attribution trailers in commits added since the plan started, not all history. C40 pins the `oha` version.
- DAG additions: C24, C21->C30, C31->C32, C16->C01.
