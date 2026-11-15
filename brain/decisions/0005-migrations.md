---
type: Decision
title: ADR-0005 Migration policy
description: Forward-only numbered migrations, checksum pinned, snapshot-tested upgrades.
tags: [decision, db, ops]
timestamp: 2026-11-07
---

# 0005 Migration policy

Forward-only `NNNN_name.sql`, no edits to applied files, destructive SQL needs an annotation, PG16 only. Enforced by `scripts/check-migrations.sh`, `crates/db/tests/upgrade.rs` (snapshots 0001/0018/0040/0050/0077, fresh == upgraded, pinned checksums) and CI job `migrations`.

Why: a self-hosted product upgrades from any past release; a silent edit to an applied file breaks them.

Details: `docs/ops/migrations.md`.
