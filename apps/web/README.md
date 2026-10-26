# bunfiiliate web

Admin and affiliate UI. Next.js (App Router) static export, TypeScript strict, Tailwind v4, shadcn-style primitives on Radix, TanStack Query, lucide icons.

The app is a pnpm workspace member (`pnpm-workspace.yaml` at the repo root, with `sdks/ts`). Install from anywhere: `pnpm install`.

## Commands

```bash
pnpm -C apps/web dev        # http://localhost:3000, api expected on :8080
pnpm -C apps/web typecheck
pnpm -C apps/web lint
pnpm -C apps/web build      # static export to apps/web/out
```

## Same-origin API

The UI calls `/api/...` on its own origin, like production (Caddy serves `out/` and proxies `/api`). The api rejects mutations whose `Origin` differs from `Host`, so the dev server must not rewrite `Host`, which Next's `rewrites` do.

`pnpm dev` runs `scripts/dev.mjs`: a small proxy on `PORT` (default 3000) sending `/api/*` to `BF_API_URL` (default `http://localhost:8080`) and everything else to `next dev` on `PORT+1`. It is not part of `next.config.ts`, so the static export is unaffected. Run the api with `COOKIE_SECURE=0` for plain-http dev, or the browser drops the `Secure` cookies.

A static host must serve `/login` from `login.html` (Caddy: `try_files {path} {path}.html /index.html`).

## Conventions

- Static export: no dynamic segments. Detail views use a query param, `?id=`, handled by `ResourceView`.
- API calls go through the generated client in `@bunfiiliate/sdk` (`src/lib/api.ts`). After the API changes, run `make openapi` and commit `sdks/ts`.
- Auth: httpOnly cookies, `credentials: include`. A 401 on any non-auth call triggers one `/api/v1/auth/refresh` and a replay; a failed refresh signs out. Route and nav guards (`src/lib/nav.ts`, `src/app/(app)/layout.tsx`) are UX only: the api enforces roles.
- Money is integer cents; format only at the edge.

## Building blocks (`src/components`)

| Piece | Use |
| :--- | :--- |
| `ResourceView` + `ResourceConfig` | A list page from one config: search, filters, `DataTable`, pagination, detail sheet (`?id=`), skeleton, empty and error states |
| `useResource(listFn, params)` (`src/lib/use-resource.ts`) | Paged query. `listFn` must be a stable (module-level) function |
| `FormDialog`, `ConfirmDialog` | Modal create/edit form and destructive confirm; they show a rejected `onSubmit`/`onConfirm` inline |
| `StatCard`, `StatusBadge` | Headline number (`accent` for the key one) and status chip |
| `placeholderResource(title, task)` | Stub page with a labelled empty state, for nav items whose API lands later |

To add a page: put a `page.tsx` under `src/app/(app)/<route>/`, add the route to `NAV` in `src/lib/nav.ts`, and render `<ResourceView config={...} />` with a `list` function that calls the generated client.

## Design

Neutral grayscale plus one accent, safety orange `#FF5A1F`, for primary actions and key numbers. Tokens live in `src/app/globals.css`; the dark theme is a `.dark` class on `<html>` (system default, toggle remembered in `localStorage`). Orange carries dark text (white fails contrast); orange text on light backgrounds uses a darker shade.
