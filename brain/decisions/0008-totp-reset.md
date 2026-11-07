---
type: Decision
title: ADR-0008 TOTP and password reset
description: TOTP 2FA with recovery codes, and a reset flow that cannot be used to enumerate accounts or brute force codes.
tags: [decision, auth, totp]
timestamp: 2026-11-07
---

# 0008 TOTP and password reset

**Decision**: RFC 6238 TOTP (sealed secret, step must be `> last_used_step`) plus single-use recovery codes stored as salted argon2 hashes. Password reset uses a hashed one-time token; accounts with TOTP also need a TOTP or recovery code. Admins can issue a reset token (A07, audit-logged).
**No oracle**: unknown email, bad code and locked account return the same status and body, with comparable timing (`password_reset.rs::unknown_email_and_no_totp_same_response_as_bad_code`). Reset locks after 5 failures per user; login backs off per (email, ip), never locking the account itself (`throttle.rs`).
**Revocation**: a reset or TOTP change bumps `users.token_version` and revokes refresh families (see [JWT ADR](/decisions/jwt-keep-handrolled.md)).
**Why**: the account recovery path is the weakest login path; make it no easier to probe than login.
Checklist: `docs/security/pentest-checklist.md` PT-07, PT-08, PT-19.
