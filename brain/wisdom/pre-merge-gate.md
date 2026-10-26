---
type: Checklist
title: Pre-Merge Gate
description: Checklist run before every feat/* -> staging merge and staging -> prod merge.
tags: [wisdom, gate, ci]
timestamp: 2026-10-26
---

# Pre-Merge Gate

## feat/* -> staging
1. Task acceptance command passes (see [Tasks](/tasks/index.md)).
2. `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace`.
3. No violation of [Anti-Patterns](/wisdom/anti-patterns.md).
4. `brain/` updated (task status + `log.md`), validated: `python ~/.claude/skills/okf/scripts/validate.py brain --strict`.
5. Short commit messages: `feat: ...`, `fix: ...`, `docs: ...`, `chore: ...`.
6. Merge with `--no-ff`.

## staging -> prod
All of the above plus T60 final gate: `make ci`, `ocr` review, `e2e` run on seeded local stack. Then push.
