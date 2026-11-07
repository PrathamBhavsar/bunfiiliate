---
type: Decision
title: ADR JWT stays hand-rolled HS256
description: Keep HMAC-SHA256 access tokens over `hmac`; harden with kid rotation, strict parsing and token_version revocation.
tags: [adr, auth, jwt]
timestamp: 2026-11-07
---

# ADR JWT stays hand-rolled HS256

**Decision**: keep the hand-rolled HS256 JWT in `api::auth` (`JwtKeys`), no `jsonwebtoken`.
**Why**: `cargo deny` fails on RUSTSEC-2023-0071 (`rsa`, no fix) for any RSA-capable JWT crate; we only need HS256. See [anti-patterns](/wisdom/anti-patterns.md).
**Hardening**: header `{alg:HS256,typ:JWT,kid}` parsed strictly (unknown fields, alg, typ, kid, size, canonical base64 all rejected); `exp` required, `nbf`/`iat` honoured; MAC via `verify_slice`; keys from `BF_JWT_SECRETS=kid:secret,...` (first signs, all verify, 32+ bytes), legacy `JWT_SECRET` is kid `legacy`.
**Revocation**: `users.token_version` (migration 0065); tokens carry `ver`, missing `ver` = 0 so sessions survive the upgrade. Checked via a per-instance cache, TTL 5 s (the revocation bound across instances), 10k entries; `auth::revoke_access_tokens` bumps and evicts.
