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
