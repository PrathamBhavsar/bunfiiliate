# Tracker contract v2 (normative)

Status: spec for A12 (routes), A13 (`t.js` v2), A14 (JS SDK). Schemas: `schemas/tracker/v2/*.schema.json` (JSON Schema 2020-12), examples in `schemas/tracker/v2/examples/`. Check with `make schemas-check`. Words MUST/SHOULD are normative.

## 1. Endpoints

| Method + path | Purpose | Auth | Notes |
| --- | --- | --- | --- |
| `GET /t.js` | tracking script (v2) | none | `Cache-Control: public, max-age=86400`, ETag, optional `?pk=<public_key>` |
| `GET /r/{code}` | click redirect | none | 302 to the code's `landing_url` (host MUST be a program site); sets `bf_click` cookie (`HttpOnly=false`, `SameSite=Lax`, 90 d); appends `bf_click=<id>` to the landing URL; unknown code: 302 to workspace fallback or 404 |
| `POST /v2/c` | browser beacon (replaces `/c`) | none, CORS `*` | schema `click-beacon`; `204` on accepted; `400` bad JSON; `413`; `503` shed |
| `OPTIONS /v2/c` | preflight | none | `204` |
| `POST /v2/events/{key}` | server-to-server event | HMAC | `key` = program public key |
| `POST /v2/webhooks/stripe/{key}` | Stripe | Stripe signature | provider-native payload |
| `POST /v2/webhooks/paddle/{key}` | Paddle | Paddle signature | provider-native payload |
| `GET /healthz`, `/readyz` | unchanged | | |

## 2. Click beacon (`POST /v2/c`)

`Content-Type: application/json` or `text/plain` (so `sendBeacon` needs no preflight).

```json
{ "v": 2, "pk": "pub_xxx", "code": "AFF123", "click": "01J...", "vid": "vid_abcdef123456", "url": "https://shop.example/pricing?x=1", "ref": "https://google.com/", "ts": 1767225600000 }
```

Rules:
- `v` required, `=2`. `pk` optional.
- At least one of `code` | `click`.
- `vid` 8..64 chars `[A-Za-z0-9_-]`.
- `url` required, <= 2048, `http`/`https` only. `ref` optional, <= 2048.
- No NUL (`\u0000`) in any string (400).
- Unknown fields are rejected (400): inbound v2 bodies are strict (`deny_unknown_fields`).
- Body <= 4 KiB (413). Response `204`, no body.

## 3. Event (`POST /v2/events/{key}`)

Headers:
- `Idempotency-Key`: REQUIRED, charset `[A-Za-z0-9_-]`, length 1..64.
- `Bf-Signature: t=<unix_secs>,v1=<hex hmac_sha256>`.
- `Content-Type: application/json`.

Signature: HMAC-SHA256 over the exact string `"{t}.{Idempotency-Key}.{raw body}"`, key = program `webhook_secret`. The key is signed because it decides deduplication. Timestamp tolerance +-300 s. Several `v1=` values MAY be present (secret rotation window); any match passes. Compare in constant time. The v1 header scheme (`x-bf-signature`/`x-bf-timestamp`) is NOT accepted on v2 routes, and `Bf-Signature` is NOT accepted on v1 routes.

Body:
```json
{
  "schema": "bf.event.v2",
  "type": "sale",
  "external_id": "ord_123",
  "occurred_at": "2026-01-01T12:00:00Z",
  "click_id": "01J...",
  "code": "AFF123",
  "customer": { "external_id": "cus_9", "email_sha256": "<64 hex>" },
  "amount_cents": 4900,
  "currency": "USD",
  "ref_external_id": "ord_123",
  "meta": { "plan": "pro" }
}
```

Rules:
- `type`: `sale | signup | lead | refund | chargeback | custom:<name>` (`<name>` = `[a-z0-9_]{1,32}`).
- `amount_cents`: integer >= 0. Floats are rejected, including integral ones (`49.0`, `4.9e3`): the server MUST reject on the number token, not after parsing to a number. Required, with `currency`, for `sale`/`refund`/`chargeback`.
- `currency`: ISO-4217, 3 uppercase letters.
- `external_id`: 1..128. `ref_external_id`: required for `refund`/`chargeback`.
- `customer.email_sha256`: lowercase hex SHA-256 of the normalized email. Raw email is never accepted (unknown field).
- `meta`: <= 20 keys, scalar values (string/number/bool), serialized <= 2 KiB.
- No `\u0000` anywhere (400).
- Unknown fields rejected (400) on this endpoint.
- Body <= 256 KiB (the existing `MAX_EVENT_BODY`); schema limits are tighter.

Responses:

| Status | Body | When |
| --- | --- | --- |
| 202 | `{"status":"accepted","id":"<uuid>"}` | first time |
| 200 | `{"status":"duplicate","id":"<uuid>"}` | same `(program, source, Idempotency-Key)`; `id` is the FIRST event's id (looked up by external id), never a new one |
| 409 | error `idempotency_conflict` | same key, different body (best effort, see 5) |
| 400 | error | validation, bad JSON, NUL |
| 401 | error | signature missing, malformed, wrong, or expired |
| 404 | error | unknown program key |
| 413 | error | body too large |
| 429 | error | per-key limiter |
| 503 | error, `Retry-After: 1` | load shed / spool full |

Error body, all `/v2` routes (schema `error`): `{"error":{"code":"snake_case","message":"human","request_id":"..."}}`.

## 4. Stripe / Paddle

Signature verification is unchanged, mapped to `/v2/webhooks/{stripe,paddle}/{key}`. Payloads are provider-native and not strictly validated (`stripe-note` documents the minimal fields read: `id`, `type`). Idempotency = provider event id. Refund/chargeback event types are defined by B01/B02.

## 5. Idempotency semantics

- DB uniqueness on `(program_id, source, external_id)` is the source of truth. `body_sha256` is stored with the row.
- The handler does no DB read per request. It uses a bounded in-memory map `(program, key) -> body_sha256` (generation-rotated, ~100k keys, ~10 min) for a fast 200/409.
- Known limitation: after a restart or eviction a same-key/different-body request is accepted at the HTTP layer and becomes a silent DB no-op; the writer compares `body_sha256` and increments `bf_idempotency_conflict_total`. Clients MUST NOT reuse a key for a different body.
- A duplicate answered from the DB path returns the first event's id.

## 6. Outbound webhooks (api -> customer)

Opt-in per endpoint: `webhook_endpoints.api_version` (text, default `v1`). v1 endpoints keep the v1 body and headers byte-for-byte. v2 endpoints get:
- Headers: `Bf-Signature: t=..,v1=..` signed over `"{t}.{body}"`, `Bf-Event-Id`, `Bf-Event-Type`, `Bf-Api-Version: 2`.
- Body (schema `outbound-envelope`): `{"id":"evt_<uuid>","api_version":"2","type":"conversion.approved","created_at":"RFC3339","data":{...}}`.
- Retries unchanged (lease delivery). Legacy `x-bf-signature`/`x-bf-timestamp` are also sent on v2 endpoints until v1 sunset (dual headers).
- Consumers MUST ignore unknown fields (outbound is additive within v2.x).

## 7. Versioning and deprecation

- v1 paths stay: `/c`, `/v1/events/{key}`, `/v1/webhooks/*`. From the release containing A13, v1 responses carry `Deprecation: true`, `Sunset: <release + 2 minor releases, HTTP-date>`, `Link: </docs/tracker-migration-v1-to-v2.md>; rel="deprecation"`. Removal only in a major bump.
- `/c` keeps working until sunset: third-party pages embed old `t.js` (cached 24 h, plus customer-pinned copies).
- Compat decisions: (1) v1 generic events keep legacy headers. (2) v1 field names stay; the v1 handler adapts into the same internal `Event` as v2 (one pipeline). (3) `bf_click` cookie name unchanged. (4) no cross-version header acceptance (no downgrade confusion). (5) v2.x changes are additive; breaking changes mean v3.
- Strictness split: inbound v2 requests reject unknown fields; outbound payloads and provider payloads tolerate them.

## 8. Test hooks

`make schemas-check` compiles every schema, requires every `*.valid.json` to pass, and every `*.invalid.json` to fail with the keyword in its filename (`<schema>.<rule>.<keyword>.invalid.json`; `lexical` = raw-token check for `49.0`). It also asserts every rule above has at least one invalid example. A missing `Idempotency-Key`, signature handling and idempotency are header/behavior rules tested in A12.
