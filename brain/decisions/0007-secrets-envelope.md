---
type: Decision
title: ADR-0007 Secrets envelope encryption
description: Provider and webhook secrets are sealed with a per-value data key wrapped by a rotating master key.
tags: [decision, security, secrets]
timestamp: 2026-11-07
---

# 0007 Secrets envelope

**Decision**: secrets at rest (`webhook_secret`, Stripe/Paddle, endpoint secrets) are `bfenc1:` values: XChaCha20-Poly1305, random data key per value, wrapped by a master key from `BF_MASTER_KEYS` (`kid:base64(32B)`, first signs, all open) or `BF_MASTER_KEY_FILE`. AAD binds the value to its table, column and row id, so a copied ciphertext does not open elsewhere. Code: `bf-core::secrets`.
**Start-up**: api, worker and tracker refuse to start without keys unless `BF_ALLOW_PLAINTEXT_SECRETS=1` (dev only). Plaintext rows still read and are sealed by `api secrets reencrypt`.
**Rotation**: new key first, `reencrypt`, `api secrets check` must exit 0, then drop the old kid. Runbook: `docs/self-hosting.md` "Secrets at rest". Secrets are shown once on create/rotate.
**Why**: a leaked dump or backup must not expose signing secrets; keys live apart from the database (back them up separately).
