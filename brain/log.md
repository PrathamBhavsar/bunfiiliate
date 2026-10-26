# Changelog

## [2026-10-26] T15 tracker.js

- **Creation**: `crates/tracker/js/tracker.src.js` + committed `tracker.min.js` (esbuild, `npm run build:js`), `scripts/size-check.mjs`, node tests in `scripts/index.js`.
- **Update**: [Tasks](/tasks/index.md) T15 done. Beacon is `application/json`; T14 `/c` needs CORS + JSON body. Learned: [Time-Traps](/wisdom/time-traps.md).

## [2026-10-26] T00 workspace scaffold

- **Creation**: Cargo workspace (bf-core, bf-db, bf-spool, bf-rt, tracker, api), Makefile, deny.toml. fmt/clippy/test/deny pass.
- **Update**: [Tasks](/tasks/index.md) T00 done. Codex blocked by hooks prompt, so Sonnet built it ([Time-Traps](/wisdom/time-traps.md)).

## [2026-10-26] Initialization

- **Creation**: bundle bootstrapped: [Overview](/overview.md), [Architecture](/architecture/index.md), [Decisions](/decisions/index.md), [Wisdom](/wisdom/index.md), [Tasks](/tasks/index.md), [Environment](/environment.md).
- Source: `docs/PLAN.md` (Opus plan), session decisions (name, AGPL-3.0, own VPS, git flow).
