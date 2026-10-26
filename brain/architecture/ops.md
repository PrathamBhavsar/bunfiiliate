---
type: Architecture
title: Ops Routes, Webhooks and Email
description: T24 outbox dispatcher, signed outbound webhooks with retry, lettre email and templates, notifications, contact requests, settings, audit log.
tags: [api, webhooks, email, outbox, notifications, audit]
timestamp: 2026-10-26
---

# Ops Routes, Webhooks and Email

Code: `crates/api/src/{dispatch,webhooks,mail,ops}.rs`, repos in `crates/db/src/ops.rs`. Conventions as in [Programs and Affiliates API](/architecture/api-routes.md): workspace-scoped, foreign id is `404`, mutations write `audit_log` in the same transaction (no secrets in diffs).

## Outbox dispatcher (worker ticks, 1s each)
- `dispatch_outbox`: claims one `outbox` row (`FOR UPDATE SKIP LOCKED`, `done_at IS NULL`), in one transaction queues a `webhook_deliveries` row per **active** endpoint of the program's workspace whose `events` is empty or contains the topic, writes the affiliate's notification, sets `done_at`. Rows without `payload.program_id` are just marked done. Email is sent after the commit, best effort (a failure is logged, never retried: at-most-once).
- `deliver_due`: claims one due delivery (`next_attempt_at <= now()`, endpoint still active), POSTs, updates the row, commit. 2xx = delivered. Otherwise backoff 30s * 2^(attempt-1) capped at 1h; after 8 attempts the delivery is dead.
- Delivery state lives in the existing columns, no migration: `attempt` = attempts made, `status_code` = last HTTP status (null before the first attempt / after a network error), `next_attempt_at` set = pending, unset + 2xx = delivered, unset otherwise = dead.
- Request: JSON `{"id": outbox_id, "topic", "data": payload}`; headers `X-Bf-Signature: t=<unix>,v1=<hex HMAC-SHA256 of "<t>.<body>">` (same scheme as the inbound generic source), `X-Bf-Event`, `X-Bf-Delivery`. 10s timeout, redirects off. Endpoint urls only need to be absolute http(s): an admin can point a delivery at an internal host (SSRF is the admin's own to avoid, same trust level as the DB).

## Email
- `Mailer::from_env`: `SMTP_URL` (lettre connection url) on = SMTP, unset = log-only. `SMTP_FROM` optional. TLS is native-tls (see [Time-Traps](/wisdom/time-traps.md)).
- Topics that mail: `conversion.approved` (key `conversion_approved`), `payout.paid` (`payout_paid`), recipient = user of `payload.affiliate_id`. Other topics (incl. `conversion.created`) only reach webhooks. `payout.paid` is not emitted yet (T18): its payload just needs `program_id` + `affiliate_id`.
- Built-in text in `mail::TEMPLATES`; a workspace row in `email_templates` (unique per key) overrides it. `{{var}}` substitution: every scalar of the payload by name, plus each `*_cents` integer as a formatted amount without the suffix (`commission_cents` 1234 -> `commission` `12.34`, integer math). Unknown var = empty, substituted values are not rescanned.
- The notification body is the rendered subject.

## Routes (`/api/v1`)
- admin: `webhooks/endpoints` CRUD (secret `whsec_...` generated, returned to admins), `webhooks/deliveries` (`?endpoint_id=`), `webhooks/deliveries/{id}/redeliver` (409 while pending), `email-templates` CRUD (key must be a known one), `contact-requests` list (`?status=`) + `PATCH {status: new|read|closed}`, `settings` GET/PUT (whole JSON object, max 16 KiB; audit logs key names only), `audit-log` (`?entity=&action=`, newest first).
- any signed-in user: `notifications` (own, `?unread=true`), `notifications/{id}/read`.
- public: `POST contact-requests` (workspace slug, optional program of that workspace, field length caps; audit entry has no actor).
