---
type: Architecture
title: Programs, Affiliates and Money API
description: T21 and T22 route surface, scoping rules, tracker NOTIFY contract, referral-code semantics, money routes.
tags: [api, programs, affiliates, referral-codes, notify]
timestamp: 2026-10-26
---

# Programs and Affiliates API

Code: `crates/api/src/{workspaces,programs,affiliates,page}.rs`, repos in `crates/db/src/{workspaces,programs,affiliates}.rs`. Built on [Auth and Tenancy](/architecture/auth.md).

## Conventions
- Lists take `?page=&limit=` (default 1/50, limit clamped 1..=200) and return `{items,page,limit,total}`.
- Every db fn takes the workspace id; handlers get it from `Principal::workspace_scope`. A foreign id is `404`, never `403`/empty. `?workspace_id=` only on `/members` (super_admin seeding a new workspace).
- Errors: `404 not_found`, `409 conflict` (unique or foreign-key violation, bad status transition). Request bodies parse through `AppJson` so errors stay `{error:{code,message}}`.
- Every mutation writes `audit_log` in the same transaction. Secrets never enter a diff.

## Routes
- super_admin: `POST/GET /workspaces`. admin: `POST/GET /members` (admins only).
- admin: programs CRUD (`PATCH` also sets `stripe_secret`/`paddle_secret`, `""` clears), `POST /programs/{id}/secrets/rotate` (new webhook secret), `/sites`, `/conversion-types`, `/plans`, `/enrollments`.
- Any role: `GET /programs[/{id}]`; affiliates see only enrolled programs and never the secrets.
- Plans: `percent` takes `rate_bps` 0..=10000 only, `flat` takes `amount_cents` >= 0 only, `x_months` needs `months >= 1`, other durations forbid `months`. Validation builds a `bf_core::CommissionPlan`.
- Affiliates: public `POST /affiliates/register` (workspace slug, creates user + `pending` affiliate), admin list/get/patch, `approve` (pending->active), `suspend` (pending|active), `reactivate` (suspended->active); `GET/PATCH /me/affiliate`.
- Codes: admin `/referral-codes`, affiliate `/me/referral-codes` (create, list, delete).

## Tracker contract
`bf_db::tracker::notify_config` (`pg_notify('bf_config','')`) runs inside the mutating transaction (delivered on commit, nothing on rollback) for: program create/update/rotate/delete, enrollment set/delete, code create/delete. Sites, plans, conversion types and affiliate status do not touch the snapshot.

## Referral-code rules
- No `active` column: **deactivate = delete the row**; the snapshot is built from existing rows. Unenrolling deletes the affiliate's codes for that program.
- Needs an `active` affiliate enrolled in the program (else `409`).
- `landing_url` is always stored (the tracker's `CodeRow.landing_url` is a non-null `String`). Default `https://<first site>`; the host must equal a program site or be a subdomain of one, no userinfo/backslash/whitespace, so `/r/:code` can't become an open redirect. A program with no sites cannot get codes.
- Code: 3-32 chars `[A-Za-z0-9_-]`, case-sensitive, globally unique; generated (10 hex) when omitted.

# Money Routes (T22)

Code: `crates/api/src/money.rs` (customers, conversions, ledger, `me/earnings`) and the route half of `payouts.rs`. Same conventions as above; all SQL joins `programs` on `workspace_id`.

- Admin only: `GET /customers[/{id}]`; `GET /conversions` (`program_id`, `affiliate_id`, `status`, `from`, `to` = UTC days on `occurred_at`, inclusive; bad date or `from > to` is `400`) and `GET /conversions/{id}`; `POST /conversions/{id}/approve|reject|reverse` (no body; forbidden transition `409`, nothing written); `GET /ledger/entries` (`program_id`, `affiliate_id`, `account`, `txn_id`; debit positive); `POST/GET /payouts/batches` (create body `{program_id}`, `409` when nobody is owed), `GET /payouts/batches/{id}` (batch + payout lines), `POST .../approve|mark-paid|cancel`, `GET .../export.csv` (`text/csv; charset=utf-8`, `Content-Disposition: attachment; filename="payouts-<id>.csv"`).
- Any role: `GET /ledger/balances` (`pending_cents`, `payable_cents` from `affiliate_balances`); an affiliate always gets their own rows, another affiliate's id is `404`.
- Affiliate only: `GET /me/earnings` (own conversions, no customer or provider ids; same filters minus `affiliate_id`), `GET /me/payouts` (own `approved` and `paid` payouts; drafts and cancelled stay internal).
- Timestamps are RFC 3339 UTC strings (SQL `to_char`), so utoipa needs no `chrono` feature.
- Audit: conversion actions write `audit_log` in the action's transaction (`conversion.approve|reject|reverse`, diff `{from,to}`). Payout actions write it right after the domain fn commits (`payout_batch.create|approve|mark_paid|cancel`): the `api::payouts` fns own their transaction, so a crash between the two loses the audit row, never money.
