# Changelog

## [2026-10-26] T13 feat/spool

- **Creation**: `bf-spool` segmented write-ahead log. Record framing `[len u32 | crc32 | payload]`, `SpoolWriter` with group-commit async queue drain and fsync modes (Always|Off), segment rotation by size, atomic checkpoint write+rename, replay iterator stopping cleanly at torn/corrupt tails, and segment GC.
- **Update**: [Tasks](/tasks/index.md) T13 marked done. Tests pass: roundtrip, torn-tail replay, corrupt CRC, checkpoint+segment GC, concurrent group commit.

## [2026-10-26] T00 workspace scaffold

- **Creation**: Cargo workspace (bf-core, bf-db, bf-spool, bf-rt, tracker, api), Makefile, deny.toml. fmt/clippy/test/deny pass.
- **Update**: [Tasks](/tasks/index.md) T00 done. Codex blocked by hooks prompt, so Sonnet built it ([Time-Traps](/wisdom/time-traps.md)).

## [2026-10-26] Initialization

- **Creation**: bundle bootstrapped: [Overview](/overview.md), [Architecture](/architecture/index.md), [Decisions](/decisions/index.md), [Wisdom](/wisdom/index.md), [Tasks](/tasks/index.md), [Environment](/environment.md).
- Source: `docs/PLAN.md` (Opus plan), session decisions (name, AGPL-3.0, own VPS, git flow).
