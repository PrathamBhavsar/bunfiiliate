---
type: TimeTrap
title: Time-Traps
description: Environment and tooling traps that already cost time on this project.
tags: [wisdom, traps, orca, rust]
timestamp: 2026-10-26
---

# Time-Traps

- **Orca `--worktree current`** means the coordinator's worktree (a different repo). Always pass `--worktree path:<abs>`.
- **Codex workers** stop at a "Hooks need review" trust prompt that Orca refuses to type into. Use claude/agy workers, or the user answers the prompt in the terminal.
- **Rust on PATH**: brew rustup is keg-only. Prepend `/opt/homebrew/opt/rustup/bin`.
- **psql on PATH is PG18** but CI is PG16. Use `/opt/homebrew/opt/postgresql@16/bin`.
- **`.mmdb` files**: GeoLite2 is not redistributable. Never commit it.
- **Static export** has no dynamic routes: use `?id=` query routes.
- **`node --test scripts/`** on Node 22 treats the dir as a module, not a glob: it needs `scripts/index.js` (tests live there).
- **Orca agent ids**: agy is `antigravity`, not `agy`. Always `worker-stop` stray test workers.
- **Orca check loop**: ack each delivery (`check --ack <id> --wait`) or old messages replay.
- **`brew services start postgresql@16` can fail silently** when PG18 holds 5432. Verify with `select version()`.
- **Parallel branches** all edit root `Cargo.toml` + `brain/log.md`: expect trivial merge conflicts, keep both sides.
- **`cargo test --workspace` requires DATABASE_URL**: Integration tests (`tests/auth.rs`, `tests/ledger.rs`, etc.) panic with `DATABASE_URL must be set` unless `DATABASE_URL=postgres://pratham@localhost:5433/postgres` is in the environment.
- **`markdownlint-cli2`**: Default MD013 (line-length 80) is unreasonable for documentation tables/diagrams; configure `.markdownlint.json` with `{"MD013": false}` and format table separators with spaces (`| --- |`).
- **`cargo test -p api payouts` filters by test fn name, not file name**: name integration tests `payouts_*` or the filter runs only unit tests (0 integration tests run, still green).
- **`tracker` `crash_before_flush_replays_exactly_once` `PoolTimedOut`** in a full `cargo test --workspace` on the shared cluster; passes with `--test-threads=2`.
- **`#[sqlx::test]` on the shared PG16 cluster**: a test binary start drops leftover `_sqlx_test_*` databases, killing other workers' live runs (`dropdb ... is being accessed by other users`, tracker/api tests fail). Not a code bug: rerun when no sibling worker is testing.
- **`cargo test -p api ops` is a name filter**, not a file selector: it runs tests whose path contains `ops`. Name every test of a task `ops_*` (integration and unit), or the acceptance command runs nothing.
- **TLS for `reqwest`/`lettre`**: default `reqwest` pulls `rustls` + `aws-lc-rs` (OpenSSL licence, fails `cargo deny`). Use `default-features = false` with `native-tls` for both (OpenSSL on Linux, Security.framework on macOS; Docker image needs libssl). lettre also needs `0BSD` allowed in `deny.toml`.
- **`cargo test --workspace metrics` is a name filter**: name every T41 test `metrics_*` (unit and integration) or it silently runs fewer. Tracker/api metrics are process-global statics, so assert `>=`/contains, never exact counts, when tests share a process.
- **`caddy validate --config <path>` adapter selection**: `caddy validate` defaults to JSON parser unless the filename ends in `Caddyfile` or `--adapter caddyfile` is explicitly passed.
- **Caddy `handle` blocks for reverse proxy and SPA routing**: Directives like `try_files` execute before `reverse_proxy` unless mutually exclusive `handle` blocks are used for route groups (`/api/*`, tracker routes `/r/*`, `/c`, `/t.js`, `/v1/events/*`, `/v1/webhooks/*`) and the fallback static file server.
- **Running `api serve` by hand** needs `DATABASE_URL JWT_SECRET API_PORT TRACKER_PORT SPOOL_DIR` (and `COOKIE_SECURE=0` over http); `bootstrap` needs them too. `api seed-demo` is still a stub: seed with the API plus SQL `INSERT ... generate_series` into `clicks`/`customers`/`conversions` (the worker's 60s rollup tick then fills `daily_rollups`). Use a throwaway DB on 5433 and drop it after.
- **`oha` deadline aborts on HTTP/1**: `oha -z <duration>` aborts in-flight requests at deadline by default and records them under `errorDistribution: {"aborted due to deadline": N}`. Pass `-w` (`--wait-ongoing-requests-after-deadline`) to drain ongoing requests cleanly.
- **psql `DROP DATABASE` multi-statement trap**: `psql` wraps multiple statements in `-c` in a transaction block, failing with `ERROR: DROP DATABASE cannot run inside a transaction block`. Execute `DROP DATABASE` and `CREATE DATABASE` in distinct `-c` invocations.
- **Generated SDK quirks**: `unread` on `GET /notifications` is typed as a required bool (send `false`, not null); `PUT /settings` body is typed `{[k]: undefined}` so a free JSON object needs a cast.
- **Browser-testing the web app on `localhost:<port>`**: cookies are not isolated by port, so another worker's dev app on `localhost` overwrites `bf_at`/`bf_rt` (different JWT secret) and every call 401s with "invalid or expired access token", bouncing you to `/login`. Open `http://<unique>.localhost:<port>` (Chrome resolves `*.localhost` to loopback; the dev proxy keeps Host, so the Origin check still passes). `api seed-demo` is not implemented: seed with `api bootstrap`, the register route, then SQL for programs/plans/enrollments/codes and `inbound_events` rows that `api serve --with-worker` ingests. Also needs `SPOOL_DIR` set.

- `#[sqlx::test]` on one shared Postgres: tracker `hot_path` failed 5 of 6 runs at the default ~10 test threads (`PoolTimedOut`, or `dropdb ... being accessed by other users`), 0 of 8 at 4 threads or fewer. `.cargo/config.toml` pins `RUST_TEST_THREADS=4`. Sibling worktrees testing on the same cluster still collide: run tests one worktree at a time.
- **Datasheet dark-mode agnosticism**: when emulating printed paper (#F4F1EA), browsers with dark mode enabled invert or force-adjust colors unless `color-scheme: light` is explicitly set in both CSS `:root` and `<meta name="color-scheme" content="light">`.
- **Inline python path resolution in shell pipes**: piping code via `python3 - << 'EOF'` sets `sys.argv[0]` to `'-'`. Resolving paths via `os.path.dirname(sys.argv[0])` defaults to empty string or parent. Pass the repo root as a CLI argument `python3 - "$ROOT_DIR" << 'EOF'` instead.
- **e2e browser install**: the `e2e` package pins `playwright-core` and ships no `playwright` bin, so `npx playwright install` fetches a different version. Use `npx e2e-web install chromium` inside `e2e/`.
- **e2e locators on this UI**: `StatusBadge` text is lowercase in the DOM but renders (and reads) capitalized, so match statuses with `{ ignoreCase: true }`. An open detail sheet (radix modal) hides the header from role queries, so `Sign out` is not found until the sheet is closed: `signOut` first opens a page without a sheet. Detail-sheet blocks have no landmark; scope with `browser.locator('section:has(> div > h3:text-is("Title"))')`. A `describe(..., { serial: true })` group keeps the same browser (cookies) and module state across its tests.
- **Caddy in tests**: `caddy run` binds the admin API on :2019 and collides with another running caddy; import `deploy/Caddyfile` from a wrapper with `{ admin off }` and set `WEB_ROOT`.

