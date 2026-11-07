---
title: Security
group: Operate
order: 8
description: Sessions, token rotation, secrets at rest, and the pre-production checklist.
---

# Security

## Sessions

Caddy serves the UI and `/api` from one origin, so there is no CORS. The access token is a 15-minute JWT in an HttpOnly, `SameSite=Strict`, `Secure` cookie (`bf_at`). The refresh token is opaque, stored as a SHA-256 hash, and rotates on every use. Presenting an already-used refresh token revokes the whole token family.

Every mutating request has its `Origin` header checked.

## Passwords and 2FA

Argon2 hashing. Optional TOTP with recovery codes. Password reset is rate-limited and does not reveal whether an account exists.

## Secrets at rest

Webhook, Stripe, Paddle and payout-rail secrets are encrypted with a master key from `BF_MASTER_KEYS`. The first key in the list encrypts; the rest decrypt older rows, so you can rotate without downtime.

## Traffic hygiene

IP velocity limits, bot user-agent filtering and click dedupe run in the tracker. IPs are stored only as salted hashes (`BF_IP_SALT`).

## Before production

- [ ] `BF_JWT_SECRETS` and `BF_MASTER_KEYS` are random and at least 32 bytes
- [ ] `BF_ALLOW_PLAINTEXT_SECRETS` is unset
- [ ] `BF_HSTS=1` once TLS works
- [ ] Metrics ports 9100 and 9101 are not public
- [ ] Backups run and one restore has been tested
- [ ] Walk through `docs/security/pentest-checklist.md`

## Reporting

See `SECURITY.md` in the repository.
