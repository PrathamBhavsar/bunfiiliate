# Changelog

## [2026-10-26] T01 ci and dependabot

- **Creation**: `.github/workflows/ci.yml` (rust with postgres:16 and cargo-deny, web conditional, js size-check conditional), `.github/dependabot.yml` (cargo, npm, github-actions weekly).
- **Update**: `Makefile` ci target mirrors Rust CI steps (fmt, clippy, test, deny).
- **Validation**: actionlint passes with 0 errors, `make ci` passes.

## [2026-10-26] T00 workspace scaffold

- **Creation**: Cargo workspace (bf-core, bf-db, bf-spool, bf-rt, tracker, api), Makefile, deny.toml. fmt/clippy/test/deny pass.
- **Update**: [Tasks](/tasks/index.md) T00 done. Codex blocked by hooks prompt, so Sonnet built it ([Time-Traps](/wisdom/time-traps.md)).

## [2026-10-26] Initialization

- **Creation**: bundle bootstrapped: [Overview](/overview.md), [Architecture](/architecture/index.md), [Decisions](/decisions/index.md), [Wisdom](/wisdom/index.md), [Tasks](/tasks/index.md), [Environment](/environment.md).
- Source: `docs/PLAN.md` (Opus plan), session decisions (name, AGPL-3.0, own VPS, git flow).
