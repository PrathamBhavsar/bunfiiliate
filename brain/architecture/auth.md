---
type: Architecture
title: Auth and Tenancy
description: Cookie JWT auth, refresh rotation with reuse detection, roles, workspace scoping.
tags: [auth, jwt, tenancy, security]
timestamp: 2026-10-26
---

# Auth and Tenancy

- Same-origin via Caddy (UI + `/api`), so no CORS.
- Access JWT 15 min in httpOnly `SameSite=Strict` cookie `bf_at`.
- Refresh token opaque httpOnly cookie `bf_rt` (path `/api/v1/auth`), rotates every use. Reuse of an old token revokes the whole family.
- Origin check on every mutation. argon2 password hashes.
- Roles: `super_admin`, `admin`, `affiliate`. `Auth<R: Role>` extractor + workspace scope on every query.
- Tenancy: workspace > program > affiliate.
- Secrets (webhook/Stripe/Paddle) are plaintext columns: trusted VPS DB, no KMS in v1.

See [System Layout](/architecture/system.md).
