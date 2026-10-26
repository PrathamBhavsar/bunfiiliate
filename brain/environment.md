---
type: Environment
title: Local Environment and Git Flow
description: Toolchain locations, git flow, and worker routing for bunfiiliate.
tags: [environment, git, orca, workers]
timestamp: 2026-10-26
---

# Local Environment and Git Flow

## Toolchain (macOS arm64)
- Rust: brew rustup, PATH `/opt/homebrew/opt/rustup/bin`. rustc 1.99.
- Postgres 16: brew service, `/opt/homebrew/opt/postgresql@16/bin`. No docker locally.
- `ocr` (alibaba/open-code-review v1.13): global npm. `e2e` (tester-army, npm `e2e`): per-project, final gate only.

## Git flow
`prod` <- `staging` <- `feat/*`. Merge `--no-ff` only. Never commit on prod/staging. Short messages (`feat: add x`). No push until final local gate passes.

## Workers (via Orca orchestration)
- Rust crates: claude Sonnet or agy (codex blocked by hooks prompt, see [Time-Traps](/wisdom/time-traps.md)).
- UI: claude Sonnet. Tests, docs, release page: agy.
- Each worker gets its own Orca worktree on a `feat/*` branch; coordinator merges.

See [Tasks](/tasks/index.md).
