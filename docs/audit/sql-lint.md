# SQL scope lint (C03)

`make sql-lint` runs `python3 -I scripts/sql-scope-lint.py` (stdlib only) after its `--self-test`.
It also runs in CI (job `rust`) and `make ci`, and in `cargo test -p api --test sql_lint`.

## Rule

Every `SELECT`/`UPDATE`/`DELETE`/`WITH` passed as a string literal to `sqlx::query*` in
`crates/db/src` and `crates/api/src` that names a tenant table must contain `workspace_id`.
Tenant tables = every table whose `CREATE TABLE` in `crates/db/migrations` has a `workspace_id`
column, plus the child tables listed in the script (`clicks`, `conversions`, `ledger_entries`,
`payouts`, `payout_batches`, `affiliates`, `customers`, `referral_codes`, `webhook_*`,
`notifications`, `audit_log`, `email_templates`, `contact_requests`, `daily_rollups`, ...).
`INSERT` is not checked.

Escape hatches (all reviewed by hand):

- `-- scope: <reason>` inside the SQL.
- `// scope: <reason>` within 3 lines above the call (or inside its args). For statements keyed by
  an id that an earlier query in the same fn already scoped.
- `docs/audit/sql-lint-allowlist.toml`: `[[allow]] file, fn, reason`. Reason is required; an entry
  that matches no flagged query is stale and fails the lint.

## What it cannot see

The lint reads only a string literal handed straight to `sqlx::query*`. It cannot see:

- **`format!`/`concat!`/const/variable-built SQL** (e.g. `query_as(&format!("... {SCOPED}"))`).
- **`query!`/`query_as!` macros.**
- A literal that names `workspace_id` but never binds or compares it (it checks presence, not logic).

Opaque SQL fails closed: it is reported as `opaque` until a human adds an allowlist entry or a
`scope:` comment. Multi-line literals (including `\` continuations and raw strings) are parsed.

Failing fixtures live in `scripts/sql-lint-fixtures/`: `format_built_sql_fails.rs`,
`query_macro_fails.rs`, `multiline_missing_scope_fails.rs`, plus the C03 cases
(`missing_scope_select_fails`, `scoped_select_passes`, `allowlisted_passes`,
`allowlist_entry_without_reason_fails`, `stale_allowlist_entry_fails`, `inline_scope_comment_passes`).

The allowlisted `format!` SQL (61 entries total) was expanded and read by hand; each carries the
workspace predicate through a const (`SCOPED`, `CUSTOMERS`, `BATCHES`, ...). The tenant matrix
(`cargo test -p api --test tenant_matrix`) is the runtime check that backs this up.
