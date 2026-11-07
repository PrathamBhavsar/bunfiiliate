---
title: Tracker
group: Reference
order: 5
description: Endpoints, click beacon, signed events, Stripe and Paddle, idempotency.
---

# Tracker

The tracker runs on port 8081. It validates every request against an in-memory snapshot and never queries Postgres while handling it. The normative contract is `docs/tracker-contract-v2.md` in the repo.

## Endpoints

| Method and path | Purpose | Auth |
| --- | --- | --- |
| `GET /t.js` | Tracking script, cached for a day, ETag | none |
| `GET /r/{code}` | Click redirect, `302` to the code's landing URL | none |
| `POST /v2/c` | Browser click beacon, `204` on accept | none, CORS `*` |
| `POST /v2/events/{key}` | Server-to-server conversion event | HMAC |
| `POST /v2/webhooks/stripe/{key}` | Stripe | Stripe signature |
| `POST /v2/webhooks/paddle/{key}` | Paddle | Paddle signature |
| `GET /healthz`, `/readyz` | Health | none |

`{key}` is the program's public key. The landing URL host must be one of the program's sites.

## Click beacon

`POST /v2/c` with JSON or `text/plain` (so `sendBeacon` needs no preflight):

```json
{ "v": 2, "pk": "pub_xxx", "code": "AFF123", "vid": "vid_abcdef123456", "url": "https://shop.example/pricing", "ref": "https://google.com/", "ts": 1767225600000 }
```

Unknown fields are rejected with `400`. Bodies over 4 KiB get `413`. When the buffer is full the tracker returns `503` instead of queueing without bound.

## Signed events

`POST /v2/events/{key}` needs two headers:

- `Idempotency-Key`: 1 to 64 characters of `[A-Za-z0-9_-]`. Required.
- `Bf-Signature: t=<unix_secs>,v1=<hex hmac_sha256>`.

Signatures are compared in constant time with a 5-minute timestamp tolerance. Replaying the same `Idempotency-Key` is safe: at-least-once delivery plus unique constraints gives effectively-once processing.

## Stripe and Paddle

Send the provider's own payload. The tracker verifies the signature (Stripe `t=,v1=`, Paddle `ts=;h1=`), spools the event after fsync and answers `200`. Business logic runs later in the worker.

## Versioning

v1 routes are deprecated. Set `BF_V1_SUNSET` to send a `Sunset` header on them. See `docs/tracker-migration-v1-to-v2.md` in the repo.
