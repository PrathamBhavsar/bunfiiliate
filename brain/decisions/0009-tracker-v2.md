---
type: Decision
title: ADR-0009 Tracker contract v2
description: Versioned /v2 tracker endpoints with schemas, HMAC events, idempotency and a v1 sunset path.
tags: [decision, tracker, api]
timestamp: 2026-11-07
---

# 0009 Tracker v2

**Decision**: the public tracker surface is versioned under `/v2` (`/v2/c`, `/v2/events/{key}`, `/v2/webhooks/*`) with JSON Schemas in `schemas/tracker/v2/`, a normative spec (`docs/tracker-contract-v2.md`) and an OpenAPI file checked against the router (`openapi_paths_match_router`). v1 stays working with `Deprecation`/`Sunset` headers (`BF_V1_SUNSET`); `/t.js` serves v2. v1 and v2 signature header schemes are never mixed on one endpoint.
**Idempotency**: event idempotency map is in memory, per tracker instance (A12b); a multi-instance deployment can double-deliver within a retry window and the worker dedups downstream.
**Compatibility rule**: changing a `/v2` field's meaning or removing a field needs a version bump. Schemas and server are held together by `make schemas-check` and the contract tests (A15). SDK: `packages/js-sdk` (A14).
**Migration**: `docs/tracker-migration-v1-to-v2.md`.
