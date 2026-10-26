---
type: Architecture
title: Web Shell and TS SDK
description: Static-export Next.js app, generated TypeScript SDK, dev proxy, auth flow, and the ResourceView building blocks.
tags: [web, sdk, nextjs, openapi]
timestamp: 2026-10-26
---

# Web Shell and TS SDK

## SDK (`sdks/ts`)
- `make openapi`: `cargo run -p api -- openapi > sdks/ts/openapi.json`, then `openapi-typescript` writes `sdks/ts/src/schema.d.ts`. `make openapi-check` regenerates into a temp dir and diffs both committed files (works before commit, unlike `git diff`).
- `createApi()` wraps `openapi-fetch` (`baseUrl ""`, `credentials: include`). Spec paths already carry `/api/v1`. Regenerate after every API-route task.

## App (`apps/web`)
- Next 16 App Router, `output: 'export'`, TS strict, Tailwind v4, Radix primitives (shadcn style), TanStack Query. `pnpm` workspace at repo root (`apps/web`, `sdks/ts`), one `pnpm-lock.yaml`; web imports the SDK as `workspace:*` (`transpilePackages` + `turbopack.root` = repo root).
- Dev: `pnpm dev` runs `scripts/dev.mjs`, a front proxy that keeps `Host` (see [Anti-Patterns](/wisdom/anti-patterns.md) 16). Not in `next.config.ts`, so export is untouched.
- Auth: cookie session; `src/lib/api.ts` wraps fetch: 401 on a non-auth call refreshes once (shared in-flight promise) then replays; failed refresh nulls the `["me"]` query, which the `(app)` layout turns into a redirect to `/login`. Role guard + sidebar come from one `NAV` table (`src/lib/nav.ts`); guard is UX only, the API enforces roles.
- Routes: admin `/dashboard /programs /affiliates /customers /conversions /ledger /payouts /analytics /reports /webhooks /email-templates /contact-requests /audit-log /settings`; affiliate `/me /me/links /me/earnings /me/payouts`; both `/notifications`. Admin pages `/programs /affiliates /referral-codes` (T31) and `/conversions /customers /ledger /payouts` (T32) are real; the rest are `placeholderResource(...)` stubs until T33-T34.
- Money pages (T32): conversion approve/reject/reverse and payout batch approve/mark-paid/cancel go through `ConfirmDialog`; batch CSV is fetched as a blob (`parseAs: "blob"`) so API errors show inline. `/ledger` switches Entries/Balances with a header toggle (two `ResourceView`s, `key` resets state). `FilterDef` has `type: "date"` (`YYYY-MM-DD`) beside selects. `useLookups` (`lib/admin.ts`) gives program/affiliate options and names; all money goes through `formatCents`.
- Blocks: `ResourceView` (config-driven list page, detail sheet on `?id=`), `useResource`, `FormDialog`, `ConfirmDialog`, `StatCard`, `StatusBadge`, `DataTable`.
- Design: grayscale + `#FF5A1F` accent, dark text on orange (white fails contrast), light/dark via `.dark` class, 13px base, mobile drawer nav at <768px.

See [Auth and Tenancy](/architecture/auth.md).
