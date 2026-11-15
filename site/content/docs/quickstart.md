---
title: Quickstart
group: Start
order: 3
description: From zero to a first click on your laptop.
---

# Quickstart

A local run, no Docker. About five minutes.

## 1. Start PostgreSQL 16

Any Postgres 16 works. Set the connection string:

Copy `.env.example` to `.env`, fill `JWT_SECRET`, `BF_MASTER_KEYS` and `BF_IP_SALT` (the file shows how to generate each), set `DATABASE_URL`, then load it in every terminal you use below:

```bash
set -a; . ./.env; set +a
```

## 2. Bootstrap the super admin

This runs migrations, creates the workspace and the first `super_admin`.

```bash
cargo run -p api -- bootstrap \
  --email admin@example.com \
  --password "use-a-long-passphrase" \
  --workspace "Acme Corp"
```

## 3. Run the API and worker

```bash
cargo run -p api -- serve --with-worker
```

The API listens on `:8080`. OpenAPI is at `http://localhost:8080/api/openapi.json`.

## 4. Run the tracker

```bash
cargo run -p tracker
```

The tracker listens on `:8081`.

## 5. Create a program and a code

In the admin UI, create a program, add your site as a program site, and create an affiliate with a referral code. Then:

```bash
curl -i http://localhost:8081/r/YOURCODE
```

You get a `302` to the landing URL with `?bf_click=<id>`. With `BF_FSYNC=always` the click is on disk before the response returns. It reaches Postgres moments later.

## Next

- Send a conversion: [Tracker](#/tracker).
- Understand the money: [Ledger](#/ledger).
- Go to production: [Deploy](#/deploy).
