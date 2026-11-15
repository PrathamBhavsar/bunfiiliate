---
title: Deploy
group: Operate
order: 7
description: One VPS with Docker Compose and Caddy, first-run bootstrap, and updates.
---

# Deploy on one VPS

A modest box is enough: 1 to 2 vCPUs, 2 GB RAM, 20 GB SSD. You need Docker Engine 24+ with Compose v2, and a DNS record pointing at the server.

## 1. Get the code and configure

```bash
git clone https://github.com/PrathamBhavsar/bunfiiliate /opt/bunfiiliate
cd /opt/bunfiiliate
cp .env.example .env
```

Edit `.env`. These are required:

```dotenv
JWT_SECRET=<openssl rand -hex 32>
POSTGRES_PASSWORD=<openssl rand -hex 16>
BF_IP_SALT=<openssl rand -hex 32>
BF_MASTER_KEYS=k1:<base64 of 32 random bytes>
SITE_ADDRESS=affiliates.example.com
```

## 2. Start

```bash
docker compose --env-file .env -f deploy/docker-compose.yml up -d
```

Run it from the repo root. `--env-file .env` is required: Compose looks for `.env` next to the compose file, not in your shell directory.

It starts four services: `postgres` (with a health check), `api` (`api serve --with-worker`), `tracker`, and `caddy` with automatic HTTPS. Migrations run when `api` starts.

## 3. First admin

```bash
docker compose --env-file .env -f deploy/docker-compose.yml exec api \
  api bootstrap --email admin@example.com --password "a-long-passphrase" --workspace "Acme Corp"
```

Then sign in at `https://affiliates.example.com/login`.

## Without Docker

systemd unit files and a Caddyfile live in `deploy/`. Run `api serve --with-worker` and `tracker` as separate units behind Caddy.

## Operate

- Metrics: Prometheus on `:9101` (api) and `:9100` (tracker). Alert rules are in `deploy/monitoring/` and `docs/ops/alerts.md`.
- Backups and restore: `deploy/backup/` and `docs/ops/backup-restore.md`.
- Disk: the tracker spool needs free disk. See `docs/ops/spool-disk.md`.
- Release checklist: `docs/ops/release-checklist.md`.
