---
type: AntiPattern
title: Banned Anti-Patterns
description: Rules that must hold in every change to bunfiiliate.
tags: [wisdom, rules]
timestamp: 2026-10-26
---

# Banned Anti-Patterns

1. Floats for money. Integer cents only.
2. Mutable balances. Derive from ledger entries.
3. UPDATE/DELETE on `ledger_entries`.
4. DB call per tracker request.
5. Unbounded channels or queues. Shed load with 503.
6. Copying code, schema text, or copy from any closed-source project (clean-room).
7. Committing directly to `prod` or `staging`. Merge `--no-ff` from `feat/*` only.
8. Pushing before the local final gate passes.
9. Committing `.env`, secrets, or the MaxMind DB. GeoIP = DB-IP lite or `CF-IPCountry`.
10. PG18-only SQL (`uuidv7()`). CI pins PG16.
11. Summing ledger legs with sequential i64 `checked_add`. Partial sums can overflow on a balanced set; sum in i128.
12. Asserting ledger balances without the sign convention (debit > 0, credit < 0). Liability accounts (AffPending, AffPayable, Cash-out) are credit-normal: balance = -sum(signed).
13. Advancing the spool checkpoint to the highest flushed offset. Handlers finish spool writes out of order; move it only over a contiguous flushed prefix, else a crash loses the in-flight gap.
14. Spool write + queue hand-off inside a cancellable handler future. A client disconnect strands a spooled record and stalls the checkpoint. Reserve the queue slot first (503 if full), then do both in a detached `tokio::spawn`.
15. `jsonwebtoken` with `rust_crypto` (or any RSA-capable JWT crate): `cargo deny` fails on RUSTSEC-2023-0071 (`rsa`, no fix). Access JWT is hand-rolled HS256 over `hmac` in `api::auth`.
16. Feeding unvalidated payloads to the retry-forever batch writer. One row Postgres rejects (e.g. a `\u0000` escape in jsonb) wedges the whole pipeline; reject at the handler (400) before spooling.
17. Storing a NULL `referral_codes.landing_url`. The tracker snapshot decodes it as a non-null `String`, so one NULL row breaks every reload. Always set it, and keep its host on the program's sites (open-redirect guard).
18. Calling `pg_notify` outside the mutating transaction. Notify inside it: delivery happens on commit and a rollback sends nothing.
19. Proxying `/api` with Next `rewrites` in dev. They replace `Host`, so the api Origin check returns 403 on every mutation. Use `apps/web/scripts/dev.mjs` (keeps `Host`).
20. `pnpm` run in a sub-package when the repo root also has an npm `package.json`: the root is always part of a pnpm workspace and gets installed too. Keep one root `pnpm-workspace.yaml`; pnpm 12 fails install on unreviewed build scripts, so list them under `allowBuilds` (`esbuild`, `unrs-resolver`: false).
21. Importing the SDK by `link:` or a path alias from `apps/web`: Turbopack refuses files outside its root and the SDK cannot find its own `node_modules`. Use `workspace:*` + `transpilePackages` + `turbopack.root`.
22. Vendoring a third-party mmdb as a test fixture (licence, size). Hand-build a one-node IPv4 tree in the test (24-bit records, `node_count=1`, left record `node_count+16+offset`, right record `node_count` = not found; all nine metadata keys are required); see `tracker::geo` tests. Test priority logic with a closure so the db path stays out of it.
23. Letting one bad inbound event abort the claim transaction and retrying it forever. Run each event in a savepoint; on a business rejection or an integrity/data (SQLSTATE 22/23) error, roll the savepoint back and set `processed_at` + `error`. Only transient DB errors leave the event unprocessed.
24. Using the typed SDK for list query params as generated: utoipa `IntoParams` without `parameter_in = Query` emits `page`, `limit`, `status`, `program_id` as path params, so `params.query` is untyped and typed calls send no query string. Web code goes through `listParams()` (sends both). Real fix: `#[into_params(parameter_in = Query)]` in the api, then `make openapi`; drop the helper after.
25. Writing client-controlled strings (referrer host) into a CSV unescaped. Quote per RFC 4180 and defuse leading `= + - @`, or the export becomes a spreadsheet formula injection.
26. Sizing a payout from the sum of approved conversions. Clawbacks (reversed paid conversions) live only in the ledger, so size it from the `aff_payable` balance minus open payouts, and shrink an open payout when one of its conversions is reversed. Never pay a negative balance.
27. Evicting expired keys by scanning a map on the tracker hot path, or short-circuiting fraud rules. Rotate key maps by generation (O(1) amortized, bounded) and run every rule on every click, or a stateful window misses clicks another rule flagged.

See [Pre-Merge Gate](/wisdom/pre-merge-gate.md).
