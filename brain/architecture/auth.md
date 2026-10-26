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

## Implemented (T20)
- `bf_at` Path=/ Max-Age=900, `bf_rt` Path=/api/v1/auth Max-Age=30d; both `HttpOnly; SameSite=Strict; Secure`. `COOKIE_SECURE=0|false` drops `Secure` for plain-http dev.
- Refresh token = 256 random bits hex; only its SHA-256 is stored. Rotation locks the row (`FOR UPDATE`); a revoked token coming back revokes the whole `family_id` and returns 401.
- Origin check: every non-GET/HEAD/OPTIONS request needs `Origin` whose authority equals `Host` (scheme ignored, missing Origin = 403). curl callers must send `-H "Origin: https://<host>"`.
- Access JWT: HS256 hand-rolled, claims `sub, role, ws, exp`; no DB hit per request.
- `Principal::workspace_scope(requested)`: super_admin may target any workspace, others only their own (else 403).
- `serve` and `bootstrap` run migrations at start. `bootstrap` refuses when a super_admin exists. Code: `crates/api`, `crates/db/src/auth.rs`.

See [System Layout](/architecture/system.md).
