# Migrating tracker v1 to v2

v1 keeps working during the deprecation window (see `docs/tracker-contract-v2.md` section 7). Migrate at your pace; v1 responses carry `Deprecation`/`Sunset` headers once A13 ships.

| v1 | v2 |
| --- | --- |
| `POST /c` | `POST /v2/c`, body gains `"v":2`, `vid` charset/length enforced, unknown fields rejected |
| `POST /v1/events/{key}` | `POST /v2/events/{key}` |
| `x-bf-timestamp` + `x-bf-signature: <hex>` | single `Bf-Signature: t=<secs>,v1=<hex>` |
| signed `"{ts}.{key}.{body}"` | signed `"{t}.{key}.{body}"` (same shape, new header) |
| `Idempotency-Key` any non-empty | `[A-Za-z0-9_-]{1,64}` |
| free-form body | `bf.event.v2` schema; integer `amount_cents` + `currency`; strict fields |
| `/v1/webhooks/{stripe,paddle}/{key}` | `/v2/webhooks/{stripe,paddle}/{key}` (same provider signatures) |
| outbound `x-bf-*` headers, v1 body | opt-in per endpoint: set `api_version` to `v2` for `Bf-*` headers and the envelope |

Steps:
1. Server events: switch the URL, replace the two signature headers with `Bf-Signature`, shorten/clean idempotency keys, send `"schema":"bf.event.v2"`, send `amount_cents` as an integer and an uppercase `currency`, send `ref_external_id` on refunds. Hash emails (SHA-256) into `customer.email_sha256`.
2. Browser: load the new `/t.js` (or the A14 SDK); it posts to `/v2/c`. Pinned old copies keep using `/c` until sunset.
3. Outbound webhooks: verify with the new headers on a test endpoint set to `v2`, then switch production endpoints. Legacy headers are sent alongside until v1 sunset; ignore unknown body fields.
4. Handle new error bodies (`error.code`) and `409 idempotency_conflict`; treat `200 duplicate` as success.

No cross-version signatures: a v1-style signature on a v2 route returns 401, and vice versa.
