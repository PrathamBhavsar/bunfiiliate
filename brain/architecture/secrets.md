---
type: Architecture
title: Secrets and Tracker Contract
description: Where secrets are sealed, who holds keys, and how the tracker contract is enforced.
tags: [secrets, tracker, security]
timestamp: 2026-11-07
---

# Secrets and tracker contract

- **Sealing**: `bf-core::secrets` (envelope, key ring) is used by the api (`crates/api/src/secrets.rs`, `api secrets reencrypt|check`), worker and tracker (decrypts at snapshot load; a program whose secrets will not open is skipped and its key answers 503, counter `bf_snapshot_decrypt_fail_total`). See [ADR-0007](/decisions/0007-secrets-envelope.md).
- **Sessions**: JWT with `kid` rotation and `token_version` revocation ([ADR](/decisions/jwt-keep-handrolled.md)); TOTP and reset in [ADR-0008](/decisions/0008-totp-reset.md); cookie rules in [Auth & Tenancy](/architecture/auth.md).
- **Tracker contract**: [ADR-0009](/decisions/0009-tracker-v2.md); spec `docs/tracker-contract-v2.md`, schemas `schemas/tracker/v2/`, OpenAPI, contract tests, JS SDK.
- **Tenant isolation**: app-layer `workspace_id` in SQL, checked by the C02 matrix and `make sql-lint`; RLS evaluated and not enabled ([ADR-0004](/decisions/0004-rls.md)).
- **Env**: `BF_MASTER_KEYS`/`BF_MASTER_KEY_FILE`, `BF_ALLOW_PLAINTEXT_SECRETS`, `BF_JWT_SECRETS`, `BF_HSTS`, `BF_V1_SUNSET` (table in README).
