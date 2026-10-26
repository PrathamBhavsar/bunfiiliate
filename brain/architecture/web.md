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
- Routes: admin `/dashboard /programs /affiliates /customers /conversions /ledger /payouts /analytics /reports /webhooks /email-templates /contact-requests /audit-log /settings`; affiliate `/me /me/links /me/earnings /me/payouts`; both `/notifications`. Pages from T31 (admin core) and T33 (analytics, reports, webhooks, email templates, contact requests, audit log, settings, notifications, dashboard) are live; the rest are `placeholderResource(...)` stubs until T32/T34.
- Charts: `components/charts.tsx` `LineChart` (hand-rolled SVG, ResizeObserver width, hover crosshair, one measure per chart, series colors `--series-1..3`); `lib/analytics.ts` hooks + `currencyFor` + CSV `downloadReport`. Detail of the table is always under a `<details>` for keyboard/screen-reader users.
- Routes: admin `/dashboard /programs /affiliates /customers /conversions /ledger /payouts /analytics /reports /webhooks /email-templates /contact-requests /audit-log /settings`; affiliate `/me /me/links /me/earnings /me/payouts /me/profile` plus public `/register`; both `/notifications`. Affiliate portal pages (T34) are live; remaining admin pages are `placeholderResource(...)` stubs until T32-T33. Portal queries use the `"portal"` key prefix, not `"me"` (anti-pattern 28).
- Blocks: `ResourceView` (config-driven list page, detail sheet on `?id=`), `useResource`, `FormDialog`, `ConfirmDialog`, `StatCard`, `StatusBadge`, `DataTable`.
- Design: grayscale + `#FF5A1F` accent, dark text on orange (white fails contrast), light/dark via `.dark` class, 13px base, mobile drawer nav at <768px.

See [Auth and Tenancy](/architecture/auth.md).
