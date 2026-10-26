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
- **`caddy validate --config <path>` adapter selection**: `caddy validate` defaults to JSON parser unless the filename ends in `Caddyfile` or `--adapter caddyfile` is explicitly passed.
- **Caddy `handle` blocks for reverse proxy and SPA routing**: Directives like `try_files` execute before `reverse_proxy` unless mutually exclusive `handle` blocks are used for route groups (`/api/*`, tracker routes `/r/*`, `/c`, `/t.js`, `/v1/events/*`, `/v1/webhooks/*`) and the fallback static file server.
