# Plan A: auth, security, secrets, tracker contract

Status: design spec (no code). Scope: TOTP password reset, secrets at rest, JWT/CSRF/headers/pentest, tracker `/v2` contract.
Workers: gemini | kiro | opencode ONLY. Never Codex. Clean-room: the live reference app is read-only for feature parity; copy nothing.
Baseline: `make ci` green. Obey `brain/wisdom/anti-patterns.md` (esp. #4 no DB per tracker request, #5 bounded queues, #9 no secrets committed, #15 no RSA JWT crate, #16 validate before spool, #17).

## 0. Global rules (every task)

- Branch from `staging`: `feat/<name>`. Merge `--no-ff` into staging only. Never commit on prod/staging. No push until local gate (`make ci`, `ocr`, `e2e`) passes. Push only to remotes under `github.com/pratham-projects`.
- Commit messages: short (`feat: add totp core`). NEVER add Co-Authored-By or any Claude/AI/tool attribution.
- Env: `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.
- No paid services. No email dependency: in-app notifications (`notifications` table), signed webhooks, optional mail behind trait, OFF by default.
- New deps must pass `cargo deny check` (licences allowed: MIT, Apache-2.0, BSD, ISC, Zlib, 0BSD, Unicode-3.0, AGPL-3.0-only). Run `cargo deny check` after each `Cargo.toml` edit.
- Migrations: next free number is `0060`. Files in `crates/db/migrations/`. Reversible by new migration only (never edit shipped ones). PG16-safe SQL (no `uuidv7()`).
- After each task: update `brain/tasks/index.md`, top of `brain/log.md`, `brain/wisdom/*` for lessons; run `python ~/.claude/skills/okf/scripts/validate.py brain --strict`.
- Each task's tests must be written FIRST (red), then code (green).
- OpenAPI: any api route change -> `make openapi-check` (regenerate `sdks/ts/openapi.json`, `schema.d.ts`).

## 1. Decisions (settled; do not relitigate)

| # | Decision | Why |
| --- | --- | --- |
| D1 | TOTP hand-rolled RFC 6238 in `bf-core::totp` over `hmac` + `sha1` (RustCrypto). QR rendered client-side from `otpauth://` URI (npm `qrcode`, MIT). | Avoids `totp-rs` dep tree; ~40 lines, RFC test vectors. |
| D2 | Cipher: XChaCha20-Poly1305 (`chacha20poly1305` crate, RustCrypto) with per-value random DEK wrapped by a KEK (envelope). | 24-byte random nonce is safe without counters; cheap KEK rotation (rewrap DEK only). |
| D3 | Keep hand-rolled HS256 JWT (anti-pattern #15). Add `kid`, multi-key verify, hardening tests. | `jsonwebtoken` pulls `rsa` (RUSTSEC-2023-0071, no fix). Spike allowed only if a crate is HS256-only and `cargo deny` clean; none is known. |
| D4 | CSRF: double-submit token (`bf_csrf` cookie, non-HttpOnly, + `X-CSRF-Token` header) IN ADDITION to existing Origin check. | Defence in depth. |
| D5 | Tracker: new `/v2/*` for machine endpoints; `/t.js` and `/r/{code}` stay unversioned (embedded in third-party pages/links); `/t.js` serves the v2 script. v1 stays 2 minor releases with `Deprecation`/`Sunset` headers. | Links/scripts in the wild must not break. |
| D6 | Mail: `Notifier` trait (in-app always; mail optional). Default impl in-app only. SMTP (existing `lettre`) stays optional. | No email dependency. |
| D7 | Password reset needs TOTP or recovery code. Users without TOTP get admin-assisted reset (one-time token). No email reset link. | Brief. |

## 2. Task DAG

```
A01 -> A02 -> A03
A04 -> A05 -> A06 -> A07
A08, A09 (independent)  A10 depends on A03,A06,A08,A09
A11 -> A12 -> A13 ; A11 -> A14 ; A12,A14 -> A15 ; A16 last (all)
```

| id | branch | worker | depends | one-line |
| --- | --- | --- | --- | --- |
| A01 | feat/secrets-crypto | gemini | - | envelope crypto in bf-core |
| A02 | feat/secrets-columns | opencode | A01 | encrypt columns, api write/read path |
| A03 | feat/secrets-tracker-rekey | kiro | A02 | tracker decrypt at load, `secrets reencrypt` CLI |
| A04 | feat/totp-core | gemini | - | TOTP + recovery code primitives |
| A05 | feat/totp-enrol | opencode | A01,A04 | enrol/confirm/disable routes |
| A06 | feat/password-reset | kiro | A05 | reset flow + lockout |
| A07 | feat/admin-reset-notify | gemini | A06 | admin-assisted reset + Notifier trait |
| A08 | feat/jwt-hardening | opencode | - | kid/rotation + tests |
| A09 | feat/csrf-headers | kiro | - | CSRF token + security headers |
| A10 | feat/pentest-suite | gemini | A03,A06,A08,A09 | checklist doc + executable tests |
| A11 | feat/tracker-v2-spec | kiro | - | contract doc + JSON Schemas |
| A12 | feat/tracker-v2-impl | opencode | A11,A03 | /v2 routes + idempotency |
| A13 | feat/tracker-v1-compat | gemini | A12 | v1 deprecation + compat tests |
| A14 | feat/js-sdk | kiro | A11 | t.js v2 + `sdks/js` |
| A15 | feat/tracker-contract-tests | opencode | A12,A14 | OpenAPI + contract tests |
| A16 | feat/plan-a-brain | gemini | all | brain + docs close-out |

Migration numbering (fixed, avoid clashes): A02=0060, A05=0061, A06=0062, A07=0063, A12=0064.

---

## 3. Part 1: secrets at rest

### A01 Envelope crypto primitive

- Branch `feat/secrets-crypto`, worker gemini, deps none.
- Files: `crates/core/Cargo.toml` (+`chacha20poly1305 = "0.10"`, `base64 = "0.22"`, `zeroize = "1"`), `crates/core/src/secrets.rs` (new), `crates/core/src/lib.rs` (`pub mod secrets;`).
- API (exact):
  ```rust
  pub struct KeyRing { active: String, keys: HashMap<String,[u8;32]> }   // zeroize on drop, Debug redacts
  impl KeyRing {
    pub fn from_env() -> Result<Self, SecretsError>;      // BF_MASTER_KEYS="k1:<b64 32B>,k2:<b64 32B>" first = active; or BF_MASTER_KEY_FILE=path (same format, one per line). Both set -> error.
    pub fn from_pairs(active:&str, pairs:&[(&str,[u8;32])]) -> Result<Self,SecretsError>;
    pub fn seal(&self, plaintext:&[u8], aad:&[u8]) -> String;              // "bfenc1:<kid>:<b64url wrapped_dek>:<b64url nonce||ct>"
    pub fn open(&self, sealed:&str, aad:&[u8]) -> Result<Vec<u8>,SecretsError>;
    pub fn needs_rewrap(&self, sealed:&str) -> bool;                       // kid != active, or not bfenc1 prefix
    pub fn rewrap(&self, sealed:&str, aad:&[u8]) -> Result<String,SecretsError>;  // new active KEK, same ciphertext+DEK? NO: re-wrap DEK only
    pub fn is_sealed(s:&str)->bool;                                         // starts_with("bfenc1:")
  }
  ```
- Format: per value, random 32B DEK, random 24B nonce. `ct = XChaCha20Poly1305(DEK).encrypt(nonce, plaintext, aad)`. `wrapped_dek = XChaCha20Poly1305(KEK).encrypt(wrap_nonce, DEK, aad)` with `wrap_nonce` prefixed to the wrapped blob. AAD = `"<table>.<column>.<row_uuid>"` bytes, binds ciphertext to its row.
- Steps: 1) write tests; 2) implement; 3) `cargo deny check`; 4) `cargo clippy -p bf-core -- -D warnings`.
- Acceptance: `cargo test -p bf-core secrets && cargo deny check`.
- Tests (bf-core, `secrets.rs` `#[cfg(test)]`):
  - unit `seal_open_roundtrip`: open(seal(x)) == x for empty, ascii, 10 KiB.
  - unit `wrong_aad_fails`: open with different aad -> `Err(Decrypt)`.
  - unit `tamper_fails`: flip one byte in each of the 4 segments -> Err.
  - unit `unknown_kid_errors`: sealed under k1, ring without k1 -> `Err(UnknownKey("k1"))`.
  - unit `rotation`: seal with ring(k1); ring(active k2, keys k1+k2) opens it; `needs_rewrap` true; after `rewrap` it is kid k2, still opens, plaintext identical; ring with only k2 opens the rewrapped one.
  - unit `two_seals_differ`: same plaintext seals to different strings.
  - unit `env_parsing`: bad base64, key != 32 bytes, duplicate kid, empty -> errors; both env+file -> error. (Use `from_str_config(&str)` helper to avoid mutating env in tests.)
  - unit `debug_redacts`: `format!("{:?}", ring)` contains no key bytes.
  - proptest `roundtrip_any_bytes`: any `Vec<u8>` (0..4096) and any aad -> roundtrip.
  - proptest `bitflip_always_detected`: flip any bit in sealed b64 payload -> never returns Ok(original).

### A02 Encrypt columns, API path

- Branch `feat/secrets-columns`, worker opencode, deps A01.
- Columns to protect: `programs.webhook_secret`, `programs.stripe_secret`, `programs.paddle_secret`, `webhook_endpoints.secret`; future provider keys follow the same rule (checklist in section 3 end). TOTP secrets (A05) use the same ring.
- Migration `crates/db/migrations/0060_secrets_encrypted.sql` (schema sketch; additive, no data loss):
  ```sql
  -- columns keep their names; value becomes 'bfenc1:...' ciphertext. Plaintext rows are legacy until re-encrypted.
  ALTER TABLE programs ADD COLUMN secrets_enc_at timestamptz;          -- NULL = may still hold plaintext
  ALTER TABLE webhook_endpoints ADD COLUMN secret_enc_at timestamptz;
  CREATE TABLE secret_rotation_runs (
    id uuid PRIMARY KEY, started_at timestamptz NOT NULL DEFAULT now(), finished_at timestamptz,
    active_kid text NOT NULL, rows_rewrapped bigint NOT NULL DEFAULT 0, rows_encrypted bigint NOT NULL DEFAULT 0
  );
  ```
- Files: `crates/api/src/lib.rs` (`AppState.keys: Arc<KeyRing>`), `crates/api/src/main.rs` (load via `KeyRing::from_env`; in `serve`/`worker` startup REFUSE to start if env missing unless `BF_ALLOW_PLAINTEXT_SECRETS=1`, logs WARN), `crates/api/src/programs.rs` (create/rotate secret handlers), `crates/api/src/webhooks.rs`, `crates/api/src/dispatch.rs` (open `e.secret` before signing at `dispatch.rs:276`), `crates/api/src/worker.rs` (any use of stripe/paddle secrets), `crates/db/src/*` repos selecting those columns, `deploy/` compose + `docs/self-hosting.md` (document `BF_MASTER_KEYS`, how to generate: `head -c32 /dev/urandom | base64`).
- Helper: `api::secrets::{seal_col(keys, table, col, row_id, plain)->String, open_col(...)->Result<String>}` building the AAD string. `open_col` accepts legacy plaintext (not `is_sealed`) and returns it (migration window), logs a WARN once per row id.
- API: no route shape changes. Responses never return secrets except once on create/rotate (existing behaviour); verify with test.
- Steps: migration; helper; wire writes (always seal); wire reads (always open); update dev/e2e env (`Makefile`, `e2e/`, `.env.example`: add a throwaway `BF_MASTER_KEYS`); docs.
- Acceptance: `cargo test -p api secrets && cargo test -p bf-db && make ci`.
- Tests (api, `crates/api/tests/secrets.rs`, `#[sqlx::test]`):
  - integration `new_program_secrets_are_ciphertext`: create program via API; `SELECT webhook_secret` raw starts with `bfenc1:` and does not contain the plaintext returned to client.
  - integration `webhook_endpoint_secret_ciphertext_and_signs`: create endpoint, trigger delivery against local test server; signature verifies with the plaintext secret.
  - integration `legacy_plaintext_still_works`: insert plaintext secret via SQL; dispatch signs correctly; row stays plaintext until reencrypt (A03).
  - integration `aad_binds_row`: copy ciphertext of program A's `webhook_secret` into program B via SQL; reading B fails (no panic, 500 logged, no secret in body).
  - integration `list_and_get_never_leak_secret`: GET program/endpoint JSON contains no `secret` substrings from DB values.
  - unit `startup_refuses_without_keys`: config load with no `BF_MASTER_KEYS` and no override -> Err.

### A03 Tracker decrypt + re-encrypt command

- Branch `feat/secrets-tracker-rekey`, worker kiro, deps A02.
- Files: `crates/tracker/Cargo.toml` (bf-core already), `crates/tracker/src/snapshot.rs` (decrypt in the loader, ONCE per reload; hot path still reads only in-memory `ProgramSecrets` plaintext in memory; zeroize old snapshot on drop is optional), `crates/tracker/src/main.rs` (load `KeyRing`), `crates/api/src/main.rs` (new subcommand `api secrets reencrypt [--dry-run]`, `api secrets check`), `docs/self-hosting.md` (rotation runbook).
- Snapshot rule: a row whose secret fails to decrypt is SKIPPED (program gets no secrets, ingest returns 503 for that key with error log), never panics, never blocks the rest of the reload. Metric `bf_snapshot_decrypt_fail_total`.
- `secrets reencrypt`: in batches of 200 rows `SELECT ... FOR UPDATE SKIP LOCKED`; for each protected column: plaintext -> `seal`; sealed with non-active kid -> `rewrap`; set `*_enc_at = now()`; commit per batch; `NOTIFY bf_config` (inside the tx, anti-pattern #18) so tracker reloads; write `secret_rotation_runs`. Idempotent. `--dry-run` only counts. `secrets check` exits 1 if any plaintext or non-active-kid row remains.
- Rotation runbook: add `k2` FIRST in `BF_MASTER_KEYS="k2:...,k1:..."` (active first) on api+tracker+worker, restart, run `api secrets reencrypt`, `api secrets check`, then remove k1.
- Acceptance: `cargo test -p tracker snapshot_secrets && cargo test -p api secrets_cli && make ci`.
- Tests:
  - integration (tracker) `snapshot_decrypts_sealed_secrets`: seeded sealed row -> `ProgramSecrets.webhook_secret` == plaintext; a signed generic event verifies.
  - integration `snapshot_skips_undecryptable_row`: one bad row + one good row -> good program loads, bad absent, metric increments, reload returns Ok.
  - integration `snapshot_accepts_legacy_plaintext`.
  - integration (api) `reencrypt_encrypts_plaintext_and_rewraps`: seed plaintext + k1-sealed rows; run with ring(active k2); all rows kid k2; `secrets check` exit 0; plaintext unchanged on decrypt; tracker notified (listen on `bf_config`).
  - integration `reencrypt_idempotent`: second run rewraps 0 rows.
  - integration `reencrypt_dry_run_changes_nothing`.
  - integration `check_fails_on_plaintext`.
  - e2e (`e2e/`): rotate keys while tracker running, event signature still accepted after reload.

Rule for new provider keys: any new column holding a credential/secret MUST (1) use `seal_col`/`open_col`, (2) be added to the `PROTECTED` const list in `crates/api/src/secrets.rs` consumed by `reencrypt`/`check`, (3) have a leak test. Add a unit test `protected_list_matches_schema` that greps `information_schema.columns` for names matching `%secret%|%api_key%|%token_enc%` and asserts each is in `PROTECTED` or on an explicit allowlist (`refresh_tokens.token_hash` etc.).

---

## 4. Part 2: TOTP password reset

Data flow: user enrols TOTP in settings -> gets 10 recovery codes once -> forgets password -> `POST /auth/password/reset {email, code, new_password}` where `code` is a 6-digit TOTP or a recovery code -> password changed, all sessions revoked. No TOTP -> admin issues one-time reset token.

### A04 TOTP + recovery primitives

- Branch `feat/totp-core`, worker gemini, deps none.
- Files: `crates/core/Cargo.toml` (+`sha1 = "0.10"`, `data-encoding = "2"` for base32; confirm `cargo deny`), `crates/core/src/totp.rs` (new), `lib.rs`.
- API:
  ```rust
  pub fn generate_secret() -> [u8;20];                        // OsRng
  pub fn base32(secret:&[u8]) -> String;                      // no padding, uppercase
  pub fn otpauth_uri(issuer:&str, account:&str, secret:&[u8]) -> String; // otpauth://totp/{issuer}:{account}?secret=..&issuer=..&algorithm=SHA1&digits=6&period=30 (percent-encoded)
  pub fn code_at(secret:&[u8], unix_secs:i64) -> u32;         // RFC 6238, SHA1, 30s, 6 digits
  pub fn verify(secret:&[u8], code:&str, now:i64, last_used_step:Option<i64>) -> Option<i64>; // accepts step-1..step+1 (skew 1), returns matched step; rejects step <= last_used_step; constant-time compare; code must be exactly 6 ASCII digits
  pub fn new_recovery_codes(n:usize)->Vec<String>;            // format "xxxxx-xxxxx" lowercase base32-crockford-ish, 50 bits each
  pub fn normalize_recovery(s:&str)->String;                  // trim, lowercase, strip spaces
  ```
- Recovery code hashing: SHA-256 of normalized code is acceptable ONLY because 50-bit random + single use + rate limit; store `sha256` bytes (reuse `hash_refresh_token` style). Document this in the module doc.
- Acceptance: `cargo test -p bf-core totp`.
- Tests:
  - unit `rfc6238_vectors`: RFC 6238 Appendix B SHA1 secret `12345678901234567890`, T=59 -> `94287082` (take last 6: `287082`), 1111111109 -> `081804`, 1111111111 -> `050471`, 1234567890 -> `005924`, 2000000000 -> `279037`, 20000000000 -> `353130`.
  - unit `skew_window`: code for step-1 and step+1 accepted at `now`; step-2 rejected.
  - unit `replay_rejected`: `verify(.., last_used_step=Some(matched))` -> None.
  - unit `rejects_non_digits_and_wrong_len`: "12345", "1234567", "12345a", " 123456", "" -> None.
  - unit `otpauth_uri_shape`: has scheme, issuer encoded (`Acme Inc` -> `Acme%20Inc`), secret base32 matches `base32()`.
  - unit `recovery_codes_unique_and_formatted`: 10 codes, regex `^[a-z2-9]{5}-[a-z2-9]{5}$`, all distinct.
  - proptest `code_always_6_digits`: any secret/time -> 0..=999_999 and formatted zero padded.
  - proptest `verify_accepts_own_code`: for any secret and now, `verify(secret, format(code_at(now)), now, None)` is Some.

### A05 Enrol / confirm / disable

- Branch `feat/totp-enrol`, worker opencode, deps A01, A04.
- Migration `0061_totp.sql`:
  ```sql
  CREATE TABLE user_totp (
    user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    secret_enc text NOT NULL,                 -- bfenc1, AAD "user_totp.secret_enc.<user_id>"
    confirmed_at timestamptz,                 -- NULL = pending enrol
    last_used_step bigint,
    created_at timestamptz NOT NULL DEFAULT now()
  );
  CREATE TABLE user_recovery_codes (
    id uuid PRIMARY KEY, user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash bytea NOT NULL, used_at timestamptz, created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, code_hash)
  );
  CREATE INDEX user_recovery_codes_user_idx ON user_recovery_codes(user_id) WHERE used_at IS NULL;
  ```
- Routes (all under `/api/v1/auth/totp`, require `Auth<AnyRole>`, covered by CSRF + Origin):
  - `POST /enrol` body `{}` -> 200 `{ "otpauth_uri": str, "secret_base32": str }`. Replaces any unconfirmed row. 409 if already confirmed.
  - `POST /confirm` body `{ "code": "123456", "current_password": str }` -> 200 `{ "recovery_codes": [10 strings] }` (shown ONCE). Verifies password (bounded argon2) AND code; sets `confirmed_at`; generates codes (replaces any older).
  - `POST /recovery-codes/regenerate` body `{ "current_password", "code" }` -> 200 `{recovery_codes}`; invalidates old.
  - `POST /disable` body `{ "current_password", "code" }` -> 204. Audit log entry `totp.disable`.
  - `GET /status` -> `{ "enrolled": bool, "recovery_codes_remaining": int }`.
- Files: `crates/db/src/totp.rs` (repo), `crates/api/src/totp.rs` (handlers), `routes.rs` (register + utoipa), `audit.rs` usage, `apps/web` settings page `security` section (TOTP enrol with `qrcode` npm package rendering `otpauth_uri`, recovery code display + "I saved them" gate).
- Steps: migration; repo; handlers; OpenAPI; web UI.
- Acceptance: `cargo test -p api totp_enrol && make openapi-check`.
- Tests (integration `#[sqlx::test]`):
  - `enrol_returns_uri_and_pending`: `status.enrolled == false` until confirm.
  - `confirm_with_valid_code_returns_10_codes_and_stores_hashes`: DB has 10 rows, `code_hash != plaintext`, `secret_enc` starts `bfenc1:`.
  - `confirm_wrong_code_or_password_401_and_not_enrolled`.
  - `confirm_replay_same_step_rejected` (uses `last_used_step`).
  - `enrol_twice_after_confirm_409`.
  - `disable_requires_password_and_code`; after disable rows deleted; audit row exists.
  - `regenerate_invalidates_old_codes`.
  - `totp_routes_require_auth_and_csrf` (401 / 403).
  - `secret_never_returned_after_enrol`: `GET /status` and `/auth/me` contain no secret.

### A06 Reset flow + lockout

- Branch `feat/password-reset`, worker kiro, deps A05.
- Migration `0062_auth_attempts.sql`:
  ```sql
  CREATE TABLE auth_throttle (
    scope text NOT NULL,                 -- 'reset_user' | 'login_user'
    key text NOT NULL,                   -- user id text (reset) or lower(email) (login)
    failures int NOT NULL DEFAULT 0, window_started_at timestamptz NOT NULL DEFAULT now(),
    locked_until timestamptz,
    PRIMARY KEY (scope,key)
  );
  ```
- Route: `POST /api/v1/auth/password/reset` (unauthenticated; Origin+CSRF check applies via double-submit cookie issued by `GET /api/v1/auth/csrf`, see A09) body:
  ```json
  { "email": "a@b.c", "code": "123456 | xxxxx-xxxxx", "new_password": "min 12 chars" }
  ```
  Responses: `204` success; `400` weak password (same rule as register; add `min 12`, max 256); `401 {"error":{"code":"reset_failed"}}` for EVERY failure cause (unknown email, no TOTP, bad code, locked) with identical body and status; `429` from IP limiter only. No hint whether the email exists.
- Logic: (1) IP limiter (existing `limit.rs`: 10/hour/IP). (2) normalise email; load user+totp; if absent run dummy work (argon2 `dummy_hash` verify + dummy HMAC) so timing is equal. (3) check `auth_throttle` for `reset_user`: >=5 failures in 15 min -> locked 15 min, doubling on repeated lock up to 24 h; locked looks identical to bad code. (4) if code matches `^\d{6}$` -> TOTP verify with `last_used_step`; else recovery path: `UPDATE user_recovery_codes SET used_at=now() WHERE user_id=$1 AND code_hash=$2 AND used_at IS NULL RETURNING id` (atomic). (5) on success in ONE tx: hash new password (bounded), `UPDATE users SET password_hash`, `DELETE FROM refresh_tokens WHERE user_id`, reset throttle, bump `users.token_version` (see A08) so existing access JWTs die, insert in-app notification `password_changed` + audit `auth.password_reset`, `outbox` webhook topic `user.password_reset` if configured. (6) never log code or password.
- Also add: login failure throttle using `auth_throttle` scope `login_user` (same thresholds, but never locks out a correct password check path that is not locked; lock returns same 401 as bad password).
- Files: `crates/api/src/password_reset.rs`, `crates/db/src/auth.rs` (+ throttle fns), `routes.rs`, web pages `apps/web/app/(auth)/forgot-password/page.tsx`.
- Acceptance: `cargo test -p api password_reset && make openapi-check && make e2e`.
- Tests:
  - integration `reset_with_totp_succeeds_and_revokes_sessions`: after reset, old refresh token -> 401, old access JWT -> 401, new password logs in, old does not.
  - `reset_with_recovery_code_succeeds_once`: second use of same code -> 401; `used_at` set.
  - `reset_unknown_email_same_response_as_bad_code`: status + body byte-identical.
  - `reset_user_without_totp_401_same_body`.
  - `totp_code_replay_rejected`: reuse of same step code fails.
  - `lockout_after_5_failures`: 5 bad codes then a CORRECT code -> 401 (locked); advance time (inject `Clock` trait or set `locked_until` in past) -> succeeds.
  - `lockout_backoff_doubles` unit on `next_lock_duration(n)`: 15m, 30m, 60m ... capped 24h.
  - `weak_password_400` and password not changed.
  - `ip_rate_limit_429_after_10`.
  - `reset_is_atomic`: inject failure after password update (test hook) -> rollback, recovery code not consumed.
  - `no_secret_in_logs`: capture tracing output during a reset; assert no code/password substring.
  - e2e `forgot_password_flow`: enrol TOTP in UI, logout, reset using generated TOTP code (test computes via `bf-core` helper exposed through a test-only CLI `api dev totp-code <secret>`), login with new password.

### A07 Admin-assisted reset + Notifier trait

- Branch `feat/admin-reset-notify`, worker gemini, deps A06.
- Migration `0063_reset_tokens.sql`:
  ```sql
  CREATE TABLE password_reset_tokens (
    id uuid PRIMARY KEY, user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash bytea NOT NULL UNIQUE, issued_by uuid NOT NULL REFERENCES users(id),
    expires_at timestamptz NOT NULL, used_at timestamptz, created_at timestamptz NOT NULL DEFAULT now()
  );
  ```
- Routes:
  - `POST /api/v1/users/{id}/password-reset-token` (admin) -> 201 `{ "token": "<43 char url-safe>", "expires_at": "RFC3339" }` shown once; TTL 1 h; replaces older unused tokens for that user. Authorisation: workspace `Admin` may target users in own workspace with role `Affiliate`/`Member`; `SuperAdmin` may target anyone except self; nobody may target self (use normal flow) or a higher/equal role except SuperAdmin; cross-workspace -> 404. Audit `auth.reset_token_issued`.
  - `POST /api/v1/auth/password/reset-with-token` body `{ "token", "new_password" }` -> 204 / generic 401 same as A06. Same side effects (revoke sessions, token_version bump, notification, audit). Same IP limiter. Token compare via SHA-256 lookup.
- Notifier trait (`crates/api/src/notify.rs`):
  ```rust
  #[async_trait] pub trait Notifier { async fn notify(&self, user_id:Uuid, kind:&str, title:&str, body:&str) -> Result<(),AppError>; }
  pub struct InApp(PgPool);            // always on: INSERT INTO notifications
  pub struct Composite(Vec<Box<dyn Notifier>>);   // InApp + optional Mail
  pub struct MailNotifier(Mailer);     // only built when BF_SMTP_URL set; uses existing mail.rs (lettre) or Resend/Brevo HTTP via reqwest behind `BF_MAIL_PROVIDER=smtp|resend|brevo`; default none
  ```
  Use `async_trait` only if already in tree; otherwise `Pin<Box<dyn Future>>` signature to avoid new dep.
- Files: `crates/api/src/notify.rs`, `crates/api/src/password_reset.rs`, `crates/api/src/mail.rs` (provider enum), `routes.rs`, web: users list "Issue reset token" button (modal shows token once with copy), `apps/web` reset-with-token page.
- Acceptance: `cargo test -p api admin_reset && cargo test -p api notify`.
- Tests:
  - `admin_issues_token_and_user_resets`: full flow, token single use (second -> 401).
  - `token_expires`: set `expires_at` past -> 401.
  - `cannot_target_self`, `cannot_target_other_workspace` (404), `affiliate_cannot_issue` (403), `admin_cannot_reset_super_admin` (403).
  - `new_token_invalidates_previous`.
  - `reset_notifies_in_app`: user has `password_changed` notification; with mail disabled no SMTP connect attempted (mock `Notifier` asserts mail impl not constructed when env unset).
  - unit `composite_continues_on_error`: failing mail notifier does not fail in-app.
  - unit `mail_provider_default_none`.
  - audit rows asserted for issue + use.

---

## 5. Part 3: JWT, CSRF, headers, pentest

### A08 JWT hardening (keep hand-rolled HS256)

- Branch `feat/jwt-hardening`, worker opencode, deps none.
- Why keep: see D3. Record in `brain/decisions/` a new ADR file `jwt-keep-handrolled.md` (frontmatter like other ADRs) with the deny finding.
- Changes in `crates/api/src/auth.rs`: header becomes `{"alg":"HS256","typ":"JWT","kid":"<id>"}`; claims add `ver` (user `token_version`), `jti`; `AppState.jwt_keys: JwtKeys {active:String, keys:HashMap<String,Vec<u8>>}` from `BF_JWT_SECRETS="k1:secret1,k2:secret2"` (first active; fallback legacy `JWT_SECRET` as kid `legacy`; min 32 bytes each, reject shorter). Verify: parse header strictly (reject if `alg != "HS256"`, `typ` present but != JWT, unknown kid, header > 256 B, token > 4 KiB, not exactly 3 segments, non-canonical b64); constant-time MAC via `Mac::verify_slice`; `exp` required and `now < exp` (leeway 0), `nbf` honoured if present, `iat` not in future > 60 s; `ver` must equal `users.token_version` (checked through a short-lived in-process cache ≤ 30 s keyed by user, bounded 10k entries, to avoid DB hit per request; reset flow evicts).
- Migration: add `ALTER TABLE users ADD COLUMN token_version int NOT NULL DEFAULT 0;` to **0062** (A06 owns the file; A08 must rebase onto it; if A08 merges first, create `0059_token_version.sql` instead and A06 drops that line. Rule: whichever merges first owns it, note in log).
- Acceptance: `cargo test -p api jwt`.
- Tests (unit in `auth.rs` + integration):
  - `alg_none_rejected`, `alg_hs512_rejected`, `alg_rs256_with_hmac_of_public_key_rejected` (classic confusion), `alg_lowercase_hs256_rejected`.
  - `expired_rejected` (exp = now-1), `exp_missing_rejected`, `nbf_future_rejected`, `iat_far_future_rejected`.
  - `bad_signature_rejected`, `truncated_signature_rejected`, `extra_segment_rejected`, `padding_in_b64_rejected`.
  - `unknown_kid_rejected`, `missing_kid_uses_legacy_only_if_configured`.
  - `rotation`: token signed k1 verifies with ring(active k2, k1,k2); after removing k1 fails; new tokens use k2 (`kid` in header).
  - `short_secret_rejected_at_startup`.
  - `role_tamper_rejected`: modify payload role to super_admin keeping signature -> 401.
  - `token_version_bump_revokes`: bump -> 401.
  - proptest `verify_never_panics`: arbitrary bytes as token -> Err, no panic.
  - proptest `roundtrip`: any principal issues then verifies equal.

### A09 CSRF + security headers

- Branch `feat/csrf-headers`, worker kiro, deps none.
- CSRF (double submit, in addition to `check_origin`): new middleware `auth::check_csrf` after `check_origin` in `lib.rs:44-47`. Cookie `bf_csrf` = 32 random bytes b64url, `Secure; SameSite=Lax; Path=/`, NOT HttpOnly, set by `login`, `refresh`, and `GET /api/v1/auth/csrf` (returns `{ "token": ... }` and sets the cookie if missing). Mutating requests (POST/PUT/PATCH/DELETE) must send header `X-CSRF-Token` equal (constant-time) to the cookie, else `403 {"error":{"code":"csrf"}}`. Plain double-submit with a random value; the Origin check remains. Exempt: `/api/v1/auth/login` is NOT exempt (login CSRF) -> web fetches `/auth/csrf` first. Exempt only: tracker (separate service), `/api/openapi.json`, health.
- Web: `apps/web/lib/api.ts` fetch wrapper reads cookie `bf_csrf`, adds header on mutations; SDK `sdks/ts` client middleware does the same.
- Headers middleware `api::headers::security_headers` on all api responses: `Strict-Transport-Security: max-age=63072000; includeSubDomains` (only when `BF_HSTS=1`, default on in prod compose), `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: strict-origin-when-cross-origin`, `Permissions-Policy: camera=(), microphone=(), geolocation=()`, `Cross-Origin-Opener-Policy: same-origin`, `Cache-Control: no-store` on `/api/v1/auth/*`. Web (Next `next.config.ts` `headers()` + Caddyfile): CSP `default-src 'self'; img-src 'self' data:; script-src 'self'; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'` (adjust only if Next needs nonces; document). Tracker `t.js`: `X-Content-Type-Options: nosniff`, CORS `Access-Control-Allow-Origin: *` only for `/v2/c` + `/c` preflight (already exists), `Cross-Origin-Resource-Policy: cross-origin` for `/t.js`.
- Acceptance: `cargo test -p api csrf && cargo test -p api headers && make e2e`.
- Tests:
  - `mutation_without_csrf_header_403`, `mismatched_csrf_403`, `valid_csrf_and_origin_ok`, `valid_csrf_but_bad_origin_403`, `get_needs_no_csrf`.
  - `login_requires_csrf`.
  - `csrf_endpoint_sets_cookie_flags` (Secure, SameSite=Lax, not HttpOnly).
  - `csrf_compare_constant_time` (uses `subtle::ConstantTimeEq` or `hmac` verify; code review item).
  - `headers_present_on_ok_and_error_responses` (200, 401, 404 all carry nosniff, XFO).
  - `auth_responses_no_store`.
  - e2e: UI login + mutation works; curl mutation w/o token 403.

### A10 Pentest checklist + executable tests

- Branch `feat/pentest-suite`, worker gemini, deps A03, A06, A08, A09.
- Files: `docs/security/pentest-checklist.md` (new), `crates/api/tests/pentest.rs` (new), `scripts/pentest.sh` (runs the Rust suite + optional live probes against `BASE_URL`), `Makefile` target `pentest`, `SECURITY.md` link. Each checklist row has: ID `PT-nn`, category, check, expected result, test name (or `manual`).
- Checklist rows (all must map to a test unless marked manual):
  - PT-01 SQL injection in every filter/query param (sort, search, ids) -> 400/empty, no 500. Test `sqli_params_do_not_500`.
  - PT-02 IDOR: user of workspace A reads/updates every resource id of workspace B -> 404. Test `idor_matrix` iterating route table.
  - PT-03 Role escalation: affiliate calls admin routes -> 403. `role_matrix`.
  - PT-04 JWT attacks (see A08 list) -> 401.
  - PT-05 Refresh token reuse -> family revoked.
  - PT-06 CSRF/Origin (A09).
  - PT-07 Login/reset enumeration: identical status/body/timing class (timing within 3x of each other over 20 samples is a smoke check, not strict) .
  - PT-08 Brute force: lockout (A06) and IP limiter.
  - PT-09 SSRF on webhook endpoint URL: 127.0.0.1, 169.254.169.254, `[::1]`, DNS rebinding hostname resolving private, redirects to private -> rejected (extends `ssrf.rs`).
  - PT-10 Open redirect on `/r/{code}` landing and any `next=` param -> only program site hosts.
  - PT-11 CSV injection (anti-pattern #25) in all exports.
  - PT-12 Secrets: no plaintext in DB (A02), none in logs/responses/OpenAPI examples. Test `db_has_no_plaintext_secrets`.
  - PT-13 Tracker: oversize body 413, malformed JSON 400, bad signature 401/400, expired timestamp rejected, replayed idempotency key deduped, queue full 503.
  - PT-14 Security headers present (A09).
  - PT-15 Cookie flags: `HttpOnly; Secure; SameSite` on `bf_at`/`bf_rt`, refresh `Path` restricted.
  - PT-16 Mass assignment: extra JSON fields (`role`, `workspace_id`, `is_admin`) ignored (`deny_unknown_fields` on admin-sensitive DTOs or explicit test).
  - PT-17 Unicode/null: `\u0000` in strings -> 400 (anti-pattern #16).
  - PT-18 Dependency audit: `cargo deny check` + `npm audit --omit=dev` (manual note; CI runs deny).
  - PT-19 TOTP: wrong/replayed/old-step codes; recovery code single-use.
  - PT-20 Password policy and Argon2 params (m>=19MiB, t>=2) asserted by parsing stored hash.
  - PT-21 manual: TLS config via Caddy (testssl.sh, free) and HSTS.
- Acceptance: `make pentest` exits 0; `cargo test -p api pentest`.
- Tests: each `PT-nn` above has one named `#[sqlx::test]`/unit test in `pentest.rs`; the checklist doc is validated by a test `checklist_rows_have_tests` that parses the markdown table and asserts every non-manual row's test name exists in `pentest.rs` source (string search).

---

## 6. Part 4: tracker `/v2` contract

### A11 Contract spec + JSON Schemas (docs only)

- Branch `feat/tracker-v2-spec`, worker kiro, deps none. No Rust changes.
- Files: `docs/tracker-contract-v2.md` (normative), `schemas/tracker/v2/{click-beacon,event,stripe-note,outbound-envelope,error}.schema.json` (JSON Schema 2020-12), `schemas/tracker/v2/examples/*.json` (valid + invalid examples, one `.invalid.json` per rule), `docs/tracker-migration-v1-to-v2.md`.
- The doc must define exactly (copy these into the spec):

**Endpoints**

| Method + path | Purpose | Auth | Notes |
| --- | --- | --- | --- |
| `GET /t.js` | tracking script (serves v2 script) | none | `Cache-Control: public, max-age=86400`, ETag, `?pk=<public_key>` optional |
| `GET /r/{code}` | click redirect | none | 302 to code's `landing_url` (host must be a program site); sets `bf_click` cookie (`HttpOnly=false`, `SameSite=Lax`, 90 d); appends `bf_click=<id>` query param to landing URL; unknown code 302 to workspace fallback or 404 |
| `POST /v2/c` | browser beacon (replaces `/c`) | none, CORS `*` | body `click-beacon` schema; `204` always on accepted; `400` bad JSON; `413`; `503` shed |
| `OPTIONS /v2/c` | preflight | none | 204 |
| `POST /v2/events/{key}` | server-to-server conversion/event | HMAC | `key` = program public key |
| `POST /v2/webhooks/stripe/{key}` | Stripe | Stripe sig | payload unchanged (provider's) |
| `POST /v2/webhooks/paddle/{key}` | Paddle | Paddle sig | same |
| `GET /healthz`, `/readyz` | unchanged | | |

**Click beacon v2** (`POST /v2/c`, `Content-Type: application/json` or `text/plain`):
```json
{ "v": 2, "pk": "pub_xxx", "code": "AFF123", "click": "01J...", "vid": "uuid-or-random-16..64", "url": "https://shop.example/pricing?x=1", "ref": "https://google.com/", "ts": 1767225600000 }
```
Rules: `v` required =2; `pk` optional; at least one of `code`|`click`; `vid` 8..64 `[A-Za-z0-9_-]`; `url` ≤ 2048 http(s) only; `ref` optional ≤ 2048; no NUL chars; body ≤ 4 KiB. Response `204` no body.

**Event v2** (`POST /v2/events/{key}`):
Headers: `Idempotency-Key: <1..128 printable ASCII>` (REQUIRED), `Bf-Signature: t=<unix_secs>,v1=<hex hmac_sha256>`, `Content-Type: application/json`.
Signed string: `"{t}.{Idempotency-Key}.{raw body}"` with HMAC key = program `webhook_secret`. Timestamp tolerance ±300 s. Multiple `v1=` allowed (secret rotation window).
Body:
```json
{
  "schema": "bf.event.v2",
  "type": "sale | signup | lead | refund | chargeback | custom:<name>",
  "external_id": "ord_123",
  "occurred_at": "2026-01-01T12:00:00Z",
  "click_id": "01J... (optional)",
  "code": "AFF123 (optional)",
  "customer": { "external_id": "cus_9", "email_sha256": "hex (optional, never raw email)" },
  "amount_cents": 4900,
  "currency": "USD",
  "ref_external_id": "ord_123 (required for refund/chargeback)",
  "meta": { "plan": "pro" }
}
```
Rules: `amount_cents` integer ≥ 0 (anti-pattern #1, floats rejected, even `49.0`), required for sale/refund/chargeback; `currency` ISO-4217 uppercase 3 letters; `external_id` 1..128; `meta` ≤ 20 keys, values string/number/bool, total ≤ 2 KiB; no `\u0000` anywhere (400, anti-pattern #16); body ≤ 64 KiB (matches `MAX_EVENT_BODY`).
Responses: `202 {"status":"accepted","id":"<uuid>"}` first time; `200 {"status":"duplicate","id":"<uuid>"}` same `(program, source, Idempotency-Key)` with same body hash; `409 {"error":{"code":"idempotency_conflict"}}` same key different body; `400` validation; `401` bad/missing/expired signature; `404` unknown key; `413`; `429` per-key limiter (existing); `503 Retry-After: 1` shed.
**Error body (all /v2):** `{"error":{"code":"snake_case","message":"human","request_id":"..."}}`.

**Stripe/Paddle:** same signature verification as now, mapped to `/v2/webhooks/...`; payload is provider-native; idempotency = provider event id.

**Outbound webhook v2 (api -> customer)**: headers `Bf-Signature: t=..,v1=..` (signed `"{t}.{body}"`), `Bf-Event-Id`, `Bf-Event-Type`, `Bf-Api-Version: 2`. Body envelope:
```json
{ "id": "evt_uuid", "api_version": "2", "type": "conversion.approved", "created_at": "RFC3339", "data": { ... } }
```
Retries unchanged (lease delivery). v1 `x-bf-signature` + `x-bf-timestamp` sent alongside until v1 sunset (dual headers).

**Versioning/deprecation:** v1 paths stay (`/c`, `/v1/events/{key}`, `/v1/webhooks/*`). From the release containing A13, v1 responses carry `Deprecation: true`, `Sunset: <date = release + 2 minor releases, RFC 7231>`, `Link: </docs/tracker-migration-v1-to-v2.md>; rel="deprecation"`. Removal in a major bump only. `/c` keeps working indefinitely until sunset because third-party pages embed old `t.js` (cached 24 h + customer-pinned copies).
**Compat decisions:** (1) v1 generic events accepted with legacy headers `x-bf-signature`/`x-bf-timestamp` unchanged. (2) v1 amount field names stay as-is; v1 handler adapts into the same internal `Event` struct as v2 (single pipeline). (3) `bf_click` cookie name unchanged. (4) v2 never accepts v1 header scheme and vice versa (no downgrade confusion). (5) New fields are additive in v2.x; consumers ignore unknown fields; breaking changes -> v3.

- Steps: write schemas; write examples; write a node script `scripts/validate-schemas.mjs` using `ajv` (MIT, dev-dep in `scripts/`) that validates every `examples/*.json` (valid ones pass, `*.invalid.json` fail); add `make schemas-check`.
- Acceptance: `make schemas-check`.
- Tests: `schemas-check` itself asserts: each schema compiles; every `*.valid.json` passes; every `*.invalid.json` fails with its expected keyword recorded in the filename (e.g. `event.float-amount.invalid.json` fails `type`); `amount 49.0` fails; missing `Idempotency-Key` is documented (header, tested in A12); a coverage check that every rule bullet above has at least one invalid example.

### A12 /v2 implementation

- Branch `feat/tracker-v2-impl`, worker opencode, deps A11, A03.
- Files: `crates/tracker/src/lib.rs` (router `lib.rs:113-127`: add `/v2/c`, `/v2/events/{key}`, `/v2/webhooks/stripe/{key}`, `/v2/webhooks/paddle/{key}`), new `crates/tracker/src/v2.rs` (DTOs w/ `serde(deny_unknown_fields)` on event, validators), `crates/tracker/src/webhook.rs` (`Source::GenericV2`; parse `Bf-Signature`, multi `v1`, tolerance 300 s), `crates/tracker/src/event.rs` (internal `Event` shared by v1+v2), `crates/db/migrations/0064_inbound_events_v2.sql`:
  ```sql
  ALTER TABLE inbound_events ADD COLUMN api_version smallint NOT NULL DEFAULT 1;
  ALTER TABLE inbound_events ADD COLUMN body_sha256 bytea;           -- idempotency conflict detection
  -- unique (program_id, source, external_id) already exists; verify name in 0001/0050 and reuse
  ```
  and `crates/api/src/worker.rs` (read `api_version`, parse v2 payload shape into the same ingest `Event`; keep v1 path).
- Idempotency: tracker computes `body_sha256`; spool record carries it; writer `INSERT ... ON CONFLICT (program_id,source,external_id) DO NOTHING RETURNING id`; if conflict, compare stored `body_sha256` -> same: duplicate (200), differs: handler cannot know synchronously (no DB per request, anti-pattern #4), so: v2 handler does an in-memory bounded LRU (generation-rotated map, anti-pattern #28, 100k keys, 10 min) of `(program,key)->body_sha256` to answer 200/409 fast; DB uniqueness is the source of truth; A conflict the LRU missed (restart, eviction) is a silent DB no-op: the writer compares `body_sha256` of the existing row and increments `bf_idempotency_conflict_total` when it differs. Document this limitation in the spec.
- Validation happens before spool (anti-pattern #16); queue slot reserved before spool, detached spawn (anti-pattern #14).
- Acceptance: `cargo test -p tracker v2 && cargo test -p api v2_ingest && make ci`.
- Tests (tracker integration, reuse existing harness):
  - `v2_event_valid_202_then_duplicate_200`: same key+body twice -> 202 then 200, exactly 1 row.
  - `v2_same_key_different_body_409` (within LRU window).
  - `v2_missing_idempotency_key_400`.
  - `v2_bad_signature_401`, `v2_expired_t_401`, `v2_future_t_401`, `v2_multi_v1_rotation_ok` (old+new secret both listed, either valid).
  - `v2_v1_header_scheme_rejected` and `v1_still_accepts_v1_headers`.
  - `v2_float_amount_400`, `v2_negative_amount_400`, `v2_bad_currency_400`, `v2_nul_char_400`, `v2_unknown_field_400`, `v2_oversize_413`, `v2_refund_requires_ref_external_id_400`.
  - `v2_beacon_204_and_one_click_row`; `v2_beacon_bad_json_400`; `v2_beacon_cors_preflight_headers`.
  - `v2_queue_full_503_with_retry_after`.
  - `v2_unknown_key_404` constant-time-ish (no timing assertion, just status).
  - proptest `v2_event_parser_total`: arbitrary bytes -> never panics; either Ok(valid per schema) or Err.
  - integration (api) `v2_sale_flows_to_conversion`: v2 sale -> worker -> conversion `pending` with correct commission (reuses `flow_click_to_approved` helpers); `v2_refund_reverses`.

### A13 v1 deprecation + compat

- Branch `feat/tracker-v1-compat`, worker gemini, deps A12.
- Files: `crates/tracker/src/lib.rs` (response map-layer on v1 routes only), `crates/tracker/src/deprecation.rs`, config `BF_V1_SUNSET` (RFC 7231 date, default set at release), metric `bf_tracker_v1_requests_total{route}`, `docs/tracker-migration-v1-to-v2.md` (finalise).
- Acceptance: `cargo test -p tracker deprecation`.
- Tests: `v1_routes_carry_deprecation_headers` (`Deprecation: true`, `Sunset`, `Link rel=deprecation`) for `/c`, `/v1/events`, `/v1/webhooks/*`; `v2_routes_have_no_deprecation_headers`; `t_js_and_redirect_have_no_deprecation_headers`; `v1_behaviour_unchanged` golden test replaying fixtures from existing tests; `v1_counter_increments`.

### A14 JS SDK + t.js v2

- Branch `feat/js-sdk`, worker kiro, deps A11.
- Files: `sdks/js/` (new package `@bunfiiliate/js`, MIT-or-repo-licence, ESM+CJS+types, zero runtime deps): `src/index.ts`, `src/signing.ts` (server-side `signEvent(secret, key, body, t?)` using `node:crypto`), `src/client.ts`, `tsconfig.json`, `package.json`; browser script source `crates/tracker/assets/tracker.src.js` + `scripts/size-check.mjs` (existing, budget min < 2048 B stays; if v2 needs more, raise to 2560 B and record in log). Existing `sdks/ts` (OpenAPI admin client) untouched.
- Public API:
  ```ts
  // browser
  bf.init({ pk?, endpoint? }); bf.track(); bf.click_id(): string|null
  // node
  new BunfiiliateServer({ baseUrl, programKey, secret }).sendEvent(event, { idempotencyKey? }): Promise<{status:'accepted'|'duplicate', id:string}>
  signEvent(secret, idempotencyKey, rawBody, t) -> "t=..,v1=.."
  verifyOutbound(secret, headers, rawBody, toleranceSecs=300): boolean   // for customers verifying our outbound webhooks
  ```
  Server client retries 429/503 with jittered backoff (max 3), reuses the same Idempotency-Key, never retries 4xx other than 429; types generated from `schemas/tracker/v2/*.json` (`json-schema-to-typescript`, dev-dep).
- Browser script behaviour: read `bf_click` from query, set cookie (90 d, `SameSite=Lax`, `Secure` on https), generate `vid` once (localStorage fallback to cookie), `navigator.sendBeacon('/v2/c', JSON)` with `fetch(keepalive)` fallback, never throws, no third-party calls, respects `navigator.doNotTrack === '1'` only when `data-respect-dnt` attribute is set.
- Acceptance: `cd sdks/js && npm test` (use `node --test` with `tsx`, or `vitest`; pick `node:test` to avoid deps) and `node scripts/size-check.mjs`.
- Tests:
  - `signEvent` matches fixed vector shared with Rust (put vector in `schemas/tracker/v2/examples/signature-vector.json`; A12 Rust test asserts the same vector).
  - `verifyOutbound` accepts valid, rejects tamper, rejects old timestamp, accepts multi `v1`.
  - `sendEvent` retries 503 twice then succeeds with same Idempotency-Key header (mock `fetch`).
  - `sendEvent` does not retry 400.
  - browser (jsdom-free: fake `document`/`navigator` objects): script sets cookie from `?bf_click=`, sends beacon with `v:2`, falls back to fetch when `sendBeacon` missing, does not throw when storage blocked.
  - size test: gzipped/minified under budget.

### A15 OpenAPI + contract tests

- Branch `feat/tracker-contract-tests`, worker opencode, deps A12, A14.
- Files: `crates/tracker/Cargo.toml` (+`utoipa` already used in api; add same version), `crates/tracker/src/openapi.rs` (OpenAPI 3.1 doc for `/v2/*`, `/t.js`, `/r/{code}`, health), route `GET /v2/openapi.json`, `docs/openapi/tracker-v2.json` (checked-in snapshot), `Makefile` target `tracker-openapi-check` (regenerates and `git diff --exit-code`), `crates/tracker/tests/contract.rs`.
- Contract tests make the JSON Schemas and Rust agree: add dev-dep `jsonschema = "0.x"` (verify `cargo deny`; if it fails, fall back to running `scripts/validate-schemas.mjs` from the test via `node` and skip with a clear message if node missing).
- Acceptance: `cargo test -p tracker contract && make tracker-openapi-check && make schemas-check`.
- Tests:
  - `every_valid_example_accepted_by_server`: iterate `schemas/tracker/v2/examples/*.valid.json` (events signed on the fly) -> 202/204.
  - `every_invalid_example_rejected_by_server`: `*.invalid.json` -> 400 (or 413 for the oversize one).
  - `server_and_schema_agree_property` (proptest): generate events from a strategy mirroring the schema; schema-valid iff server accepts.
  - `openapi_paths_match_router`: set of `(method,path)` in spec == set of registered v2 routes (route table exposed by a `routes()` const).
  - `signature_vector_matches_js` (shared vector).
  - `error_body_matches_error_schema` for 400/401/404/409/413/503.
  - `openapi_snapshot_not_drifted`.

### A16 Close-out

- Branch `feat/plan-a-brain`, worker gemini, deps all.
- Update `brain/tasks/index.md` (A01..A15 rows), `brain/log.md`, `brain/architecture/` (auth + secrets + tracker contract pages), `brain/decisions/` (ADRs: secrets envelope, totp-reset, tracker-v2, jwt keep), `brain/wisdom/anti-patterns.md` (append new bans below), `SECURITY.md`, `docs/self-hosting.md`, `README.md` env table.
- Acceptance: `python ~/.claude/skills/okf/scripts/validate.py brain --strict && make ci && make pentest && make e2e`.

New anti-patterns to append (A16):
31. Storing or logging a TOTP secret, recovery code, reset token, or provider secret in plaintext. Seal with the key ring; hash tokens/codes.
32. Distinguishable reset/login failures (status, body, or timing) for unknown email vs bad code vs locked.
33. Accepting a TOTP step `<= last_used_step`.
34. Sealing a secret without row-bound AAD.
35. Removing a KEK id before `api secrets check` exits 0.
36. Accepting a JWT without strict `alg`/`kid`/`exp` checks, or mixing v1 and v2 signature header schemes on one endpoint.
37. Changing a `/v2` payload field meaning or removing a field without a version bump.

## 7. Env / config added (document in README + self-hosting)

| Var | Used by | Meaning |
| --- | --- | --- |
| `BF_MASTER_KEYS` / `BF_MASTER_KEY_FILE` | api, worker, tracker | `kid:base64(32B)` list, first active |
| `BF_ALLOW_PLAINTEXT_SECRETS` | api, tracker | dev only, default unset |
| `BF_JWT_SECRETS` | api | `kid:secret` list, first active (legacy `JWT_SECRET` fallback) |
| `BF_HSTS` | api | `1` to send HSTS |
| `BF_V1_SUNSET` | tracker | HTTP date for Sunset header |
| `BF_MAIL_PROVIDER` | api | `none` (default) / `smtp` / `resend` / `brevo` |

## 8. Risks / open items

- A08 and A06 both touch `users.token_version` migration: whichever merges first owns it (see A08).
- A12 cannot detect "same key, different body" across restarts without a DB call; DB uniqueness keeps correctness, the 409 is best effort inside the LRU window (documented).
- Tracker `t.js` size budget may need raising (A14).
- `chacha20poly1305`, `sha1`, `data-encoding`, `zeroize`, `jsonschema` must clear `cargo deny`; fallbacks: `aes-gcm` (same org), hand-rolled base32 (20 lines), skip `zeroize`, node-based schema check.
- Live reference app used only for feature parity checks (affiliate portal reset UX); nothing copied.
