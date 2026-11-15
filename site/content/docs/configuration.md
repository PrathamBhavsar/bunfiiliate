---
title: Configuration
group: Operate
order: 4
description: Environment variables for api, worker and tracker.
---

# Configuration

Everything is an environment variable. `.env.example` in the repo lists the full set with comments.

## Required in production

| Variable | Used by | Meaning |
| --- | --- | --- |
| `DATABASE_URL` | api, worker, tracker | Postgres 16 connection string |
| `BF_JWT_SECRETS` | api | `kid:secret` list, the first entry signs. Legacy `JWT_SECRET` is still read. At least 32 bytes. |
| `BF_MASTER_KEYS` or `BF_MASTER_KEY_FILE` | api, worker, tracker | `kid:base64(32 bytes)` list, first is active. Encrypts webhook and provider secrets. |
| `BF_IP_SALT` | tracker | Salt for IP hashing. Required. |
| `SITE_ADDRESS` | caddy | Public https hostname for automatic TLS |

Generate secrets:

```bash
openssl rand -hex 32
echo "k1:$(head -c32 /dev/urandom | base64)"
```

## Optional

| Variable | Used by | Meaning |
| --- | --- | --- |
| `BF_FSYNC` | tracker | `always` (default, durable) or `off` (page cache only, faster, can lose recent clicks on host crash) |
| `BF_HSTS` | api | `1` sends HSTS |
| `BF_V1_SUNSET` | tracker | HTTP date for the v1 `Sunset` header |
| `SMTP_URL` | api | Enables mail: password reset and notifications |
| `SMTP_FROM` | api | From address |
| `BF_ALLOW_PLAINTEXT_SECRETS` | api, tracker | Development only. Leave unset in production. |

## Ports

| Port | Service |
| --- | --- |
| 8080 | api |
| 8081 | tracker |
| 9100 | tracker Prometheus metrics (internal) |
| 9101 | api Prometheus metrics (internal) |

## Durability mode

`BF_FSYNC=always` makes the tracker fsync a group of requests before acknowledging any of them. `off` skips that flush. See the [benchmarks](https://github.com/PrathamBhavsar/bunfiiliate/blob/staging/docs/benchmarks.md) for both modes and the hardware they ran on.
