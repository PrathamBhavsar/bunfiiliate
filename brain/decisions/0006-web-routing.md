---
type: Decision
title: ADR-0006 Web routing for static export
description: Detail pages use query-string routes (?id=, ?code=), not dynamic path segments.
tags: [decision, web, ops]
timestamp: 2026-11-07
---

# 0006 Web routing

`output: "export"` cannot serve `/x/[id]` for runtime ids without `generateStaticParams`. Options:

- (a) **Query-string routes** `/x/detail?code=...`. Chosen. No server, works with Caddy static files.
- (b) Dynamic Next server. Rejected: adds a Node runtime.
- (c) Hash routes. Rejected: not visible to the server, worse for links.

Rules: read params with `useSearchParams()` inside `<Suspense>`. Keep `trailingSlash` unset (false) so the export emits `x/detail.html`, which matches Caddy `try_files {path} {path}.html {path}/ /index.html` in `deploy/Caddyfile`. No `next.config.ts` change needed.

Page table: `docs/ui/routes.md`. Guard: `scripts/check-static-export.sh` (run after `make web`).
