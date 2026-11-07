# Self-Hosting bunfiiliate

This guide covers deploying and operating bunfiiliate on a single Virtual Private Server (VPS) using Docker Compose or bare-metal systemd.

---

## 1. System Requirements & Architecture

bunfiiliate is designed to run efficiently on a single modest VPS (e.g. 1–2 vCPUs, 2 GB RAM, 20 GB SSD).

### Architecture Overview

```text
               Internet (Clients / Affiliates / Webhooks)
                                  │
                                  ▼
                 ┌─────────────────────────────────┐
                 │        Caddy (Port 80/443)      │
                 │   • Automatic TLS (Let's Encrypt)│
                 │   • Serves static web UI        │
                 │   • Same-origin reverse proxy   │
                 └───────┬─────────────────┬───────┘
                         │                 │
            /api/*       │                 │  /r/*, /c, /t.js,
                         ▼                 │  /v1/events/*, /v1/webhooks/*
              ┌─────────────────────┐      ▼
              │      API Server     │  ┌─────────────────────┐
              │     (Port 8080)     │  │       Tracker       │
              │  api serve          │  │     (Port 8081)     │
              │  --with-worker      │  │  In-memory snapshot │
              └──────────┬──────────┘  │  WAL spool writer   │
                         │             └──────────┬──────────┘
                         │                        │
                         │ Ingest / Queries       │ Batch Flushes
                         ▼                        ▼
                 ┌─────────────────────────────────┐
                 │       PostgreSQL 16 Database    │
                 │      (Single source of truth)   │
                 └─────────────────────────────────┘
```

- **Caddy**: Serves the Next.js static export and reverse-proxies `/api` and tracker routes under a single domain. This same-origin architecture avoids CORS overhead and protects `SameSite=Strict` HTTP-only session cookies.
- **Tracker**: Ingests clicks and conversions without per-request database queries, writing records to a write-ahead log (WAL) spool before batching into Postgres.
- **API + Worker**: Serves administrative and affiliate portal APIs, while the integrated worker (`--with-worker`) processes inbound spooled events, performs fraud checks, updates the financial ledger, dispatches outbox webhooks/emails, and rolls up daily analytics.
- **PostgreSQL 16**: Relational storage and immutable financial ledger. Note: PostgreSQL 16 is required; UUIDv7 identifiers are generated application-side in Rust.

---

## 2. Docker Compose Deployment (Recommended)

### Prerequisites

- A Linux VPS (Ubuntu 22.04+, Debian 12+, Rocky Linux, etc.)
- Docker Engine (24+) and Docker Compose (v2+)
- A public DNS record (A or AAAA) pointing your domain to the VPS IP

### Step 1: Clone Repository & Prepare Environment

```bash
git clone https://github.com/bunfiiliate/bunfiiliate.git /opt/bunfiiliate
cd /opt/bunfiiliate

# Copy example environment configuration
cp .env.example .env
```

Edit `.env` to configure your production settings:

```dotenv
# Generate a random secret with: openssl rand -hex 32 (at least 32 bytes)
JWT_SECRET=your-secure-random-secret-key-at-least-32-chars

# PostgreSQL password (used inside DATABASE_URL, so keep it URL-safe: openssl rand -hex 16)
POSTGRES_PASSWORD=your-strong-db-password

# Secret salt for IP hashing in the tracker (required): openssl rand -hex 32
BF_IP_SALT=your-random-ip-salt

# Master key(s) that encrypt webhook/Stripe/Paddle secrets in the database (required).
# Format kid:base64(32 bytes); the first entry is the active key.
# Generate: echo "k1:$(head -c32 /dev/urandom | base64)"
BF_MASTER_KEYS=k1:replace-with-generated-value

# Domain for Caddy automatic TLS (required, an https hostname)
SITE_ADDRESS=affiliates.example.com

# Optional outbound SMTP
# SMTP_URL=smtps://user:password@smtp.example.com:465
# SMTP_FROM="bunfiiliate <noreply@example.com>"
```

### Step 2: Start the Stack

Run the stack in the background:

```bash
docker compose --env-file .env -f deploy/docker-compose.yml up -d
```

Run it from the repo root. Compose looks for `.env` next to the compose file (`deploy/`), not in your shell's directory, so `--env-file .env` is required for the root `.env` to be read. `JWT_SECRET`, `POSTGRES_PASSWORD`, `BF_IP_SALT`, `BF_MASTER_KEYS` and `SITE_ADDRESS` have no defaults: Compose refuses to start if any is unset. `deploy/docker-compose.yml` is the one canonical file; `deploy/compose.yml` is a symlink to it. Check your configuration with `docker compose --env-file .env -f deploy/docker-compose.yml config`.

Compose launches four services:

1. `postgres`: PostgreSQL 16 database with health check
2. `api`: `api serve --with-worker` (HTTP API and background tasks)
3. `tracker`: `tracker` ingestion daemon
4. `caddy`: Edge server with automatic HTTPS

Database schema migrations execute automatically when the `api` service starts.

---

## Payout rails

`POST /api/v1/payouts/batches/{id}/mark-paid` with no body marks a batch paid by hand (manual CSV). With `{"rail": "mock" | "paypal_payouts" | "wise"}` each payout line goes to a provider instead.

- Credentials are per workspace, not env vars: `PUT /api/v1/payouts/rails/credentials` with `{"rail": "paypal_payouts", "sandbox": true, "credentials": {...}}` (`DELETE ...?rail=` removes them). PayPal: `client_id`, `client_secret`. Wise: `api_token`, `profile_id`. They are sealed with the master key (like webhook secrets), never returned, and `GET /api/v1/payouts/rails` shows only whether each rail is configured. A rail with no credentials answers `422`; `mock` is always available.
- Line states: `pending -> submitted -> paid | failed`. No provider call runs inside a database transaction. The provider idempotency key is the payout line id, so retrying `mark-paid` re-sends failed lines (or lines whose outcome is unknown) without paying twice. The ledger posts only when the provider confirms `paid`; the batch becomes `paid` once every line is.
- `POST /api/v1/payouts/batches/{id}/reconcile` asks the provider about lines it accepted but has not settled (PayPal and Wise settle asynchronously) and posts the confirmed ones. Run it until the batch is paid. While a line is in flight, marking the batch paid by hand is refused.
- PayPal Payouts has a sandbox and a pay-as-you-go live tier. Wise: the adapter follows the documented quote, recipient, transfer, fund flow, but has not been run against the Wise sandbox, and live use needs a Wise business agreement and supported email-recipient currencies. Treat it as untested until you have verified it in your sandbox. Zero-decimal currencies (JPY, KRW, ...) are refused by both adapters.

## 3. First-Run Bootstrap

Before logging in, initialize the root workspace and super-administrator account using `api bootstrap`:

```bash
docker compose --env-file .env -f deploy/docker-compose.yml exec api \
  api bootstrap \
  --email admin@example.com \
  --password "YourSecureAdminPassword123!" \
  --workspace "Acme Corp"
```

The bootstrap command:

1. Connects to PostgreSQL using `DATABASE_URL`
2. Runs all pending migrations
3. Creates the workspace
4. Hashes the password using Argon2id
5. Inserts the `super_admin` user record and prints `created super_admin <uuid>`

You can now log in at `https://affiliates.example.com/login`.

---

## 4. TLS & Domain Configuration

Caddy provisions and renews TLS certificates automatically from Let's Encrypt or ZeroSSL.

Ensure:

- Ports `80` (HTTP) and `443` (HTTPS) are open in your VPS firewall (`ufw allow 80/tcp && ufw allow 443/tcp`).
- Your domain's DNS `A`/`AAAA` record points directly to the server.
- The `SITE_ADDRESS` variable in your environment matches your public domain (e.g. `affiliates.example.com`).

Auth cookies are `Secure`, so the dashboard only works over HTTPS. For local testing over plain HTTP only, set `SITE_ADDRESS=:80` and `COOKIE_SECURE=0`; without `COOKIE_SECURE=0` the browser drops the cookies and login silently bounces back to `/login`. Never use `COOKIE_SECURE=0` in production.

### Same-Origin Security Model

Caddy routes requests as follows:

- `/r/*`, `/c`, `/t.js`, `/v1/events/*`, `/v1/webhooks/*` -> `tracker:8081`
- `/api/*` -> `api:8080`
- All other paths -> `/srv/web` (Next.js static export with HTML fallback)

Because the web frontend and API share the exact same origin, the API enforces strict security controls:

- Mutating requests (`POST`, `PUT`, `DELETE`) verify the `Origin` header matches the request host.
- Auth cookies (`bf_at` for 15-minute access JWT, `bf_rt` for rotating refresh token) use `SameSite=Strict`, `httpOnly`, and `Secure`.

---

## 5. GeoIP & Cloudflare Resolution

The tracker assigns country codes to incoming click and conversion traffic:

1. **Cloudflare**: If traffic routes through Cloudflare, Cloudflare appends the `CF-IPCountry` and `CF-Connecting-IP` headers. Set `BF_TRUST_CF=1` so the tracker reads them with zero external database dependencies, and delete the two `request_header -CF-...` lines in `deploy/Caddyfile`. Without Cloudflare, leave `BF_TRUST_CF` unset: the Caddyfile strips client-supplied `CF-Connecting-IP` and `CF-IPCountry` from tracker routes so a client cannot forge its IP or country. `BF_TRUSTED_PROXIES` lists the proxies whose `X-Forwarded-For` the tracker believes (default: none, so the client is the TCP peer). The compose file defaults it to Docker's private ranges so Caddy's forwarded address is used; without a trusted proxy every click would hash to the proxy's address. For a systemd install with a reverse proxy on the same host, set `BF_TRUSTED_PROXIES=127.0.0.1/32`. Generic conversion events (`POST /v1/events/:key`) are signed with HMAC-SHA256 over `<X-Bf-Timestamp>.<Idempotency-Key>.<body>` (hex in `X-Bf-Signature`, timestamp within 5 minutes).
2. **Direct Edge (DB-IP Lite)**: When running directly on the Internet without Cloudflare, download the free DB-IP Lite country database:

```bash
./scripts/fetch-dbip.sh
```

Then configure `BF_GEOIP_DB` in your `.env`:

```dotenv
BF_GEOIP_DB=data/dbip-country-lite.mmdb
```

> **Note on MaxMind**: Do not commit or vendor MaxMind GeoLite2 databases; use DB-IP Lite or the Cloudflare header.

---

## 6. Bare-Metal & Systemd Installation

For environments without Docker, run bunfiiliate directly on Debian/Ubuntu with `systemd` and PostgreSQL.

### Step 1: Install System Dependencies

```bash
# PostgreSQL 16
sudo apt-get update
sudo apt-get install -y postgresql-16 curl ca-certificates

# Caddy
sudo apt-get install -y debian-keyring debian-archive-keyring apt-transport-https
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt-get update
sudo apt-get install -y caddy
```

### Step 2: Build Binaries and Web Export

```bash
# Build Rust binaries
cargo build --release --bin api --bin tracker
sudo cp target/release/api /usr/local/bin/
sudo cp target/release/tracker /usr/local/bin/

# Build static frontend export
pnpm install --frozen-lockfile
pnpm -C apps/web build
sudo mkdir -p /var/www/bunfiiliate
sudo cp -r apps/web/out/* /var/www/bunfiiliate/
```

### Step 3: Configure Dedicated User & Directories

```bash
sudo useradd -r -s /bin/false -d /var/lib/bunfiiliate bunfiiliate
sudo mkdir -p /var/lib/bunfiiliate /var/spool/bunfiiliate /etc/bunfiiliate
sudo chown -R bunfiiliate:bunfiiliate /var/lib/bunfiiliate /var/spool/bunfiiliate
```

Write `/etc/bunfiiliate/bunfiiliate.env`:

```dotenv
DATABASE_URL=postgres://bunfiiliate:secret@localhost:5432/bunfiiliate
API_PORT=8080
TRACKER_PORT=8081
JWT_SECRET=your-secure-random-secret-key-at-least-32-chars
SPOOL_DIR=/var/spool/bunfiiliate
RUST_LOG=info
```

### Step 4: Install Systemd Services

Copy the provided service units from `deploy/systemd/`:

```bash
sudo cp deploy/systemd/bunfiiliate-api.service /etc/systemd/system/
sudo cp deploy/systemd/bunfiiliate-tracker.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now bunfiiliate-api bunfiiliate-tracker
```

### Step 5: Configure Caddy

Place `deploy/Caddyfile` into `/etc/caddy/Caddyfile` (adjusting `/srv/web` to `/var/www/bunfiiliate` and `api:8080`/`tracker:8081` to `localhost:8080`/`localhost:8081`), then reload:

```bash
sudo systemctl reload caddy
```

---

## 7. Backups & Disaster Recovery

### PostgreSQL Database Backup

The PostgreSQL database contains all configuration, user credentials, conversion records, and the double-entry financial ledger.

Create a nightly compressed backup using `pg_dump`:

```bash
# Docker Compose
docker compose --env-file .env -f deploy/docker-compose.yml exec -T postgres \
  pg_dump -U postgres bunfiiliate | gzip > /backups/bunfiiliate-$(date +%F).sql.gz

# Bare metal
pg_dump -U bunfiiliate bunfiiliate | gzip > /backups/bunfiiliate-$(date +%F).sql.gz
```

Restore with:

```bash
gunzip -c /backups/bunfiiliate-2026-10-26.sql.gz | \
  docker compose --env-file .env -f deploy/docker-compose.yml exec -T postgres psql -U postgres -d bunfiiliate
```

### Secrets at rest and master keys

Webhook signing secrets, Stripe/Paddle secrets and endpoint secrets are stored encrypted (`bfenc1:` values: XChaCha20-Poly1305, a random data key per value wrapped by a master key). The API, worker and tracker all need `BF_MASTER_KEYS` (or `BF_MASTER_KEY_FILE`, same format, one key per line; set only one). Without it they refuse to start, unless `BF_ALLOW_PLAINTEXT_SECRETS=1`, which stores secrets in plaintext and logs a warning.

**Back the keys up separately from the database dump.** A dump restored without its keys cannot be decrypted.

Existing plaintext rows keep working and are encrypted by the re-encrypt command below.

**Rotating the master key**

1. Generate a new key and put it first, keeping the old one: `BF_MASTER_KEYS="k2:<new>,k1:<old>"` on the api, worker and tracker. Restart them.
2. `api secrets reencrypt` seals any plaintext and re-wraps every value under `k2`. Use `--dry-run` to count first. It is idempotent and tells the tracker to reload.
3. `api secrets check` exits 1 while any plaintext or old-key value remains. Run it until it exits 0.
4. Remove `k1` from `BF_MASTER_KEYS` and restart.

The tracker skips a program whose secrets will not decrypt (`bf_snapshot_decrypt_fail_total` counts them) and answers 503 for that program's key until the keys are fixed; other programs are unaffected.

### WAL Spool Directory

The spool directory (`/var/spool/bunfiiliate` or volume `spool_data`) stores in-flight event records. Segments are deleted only after the batch writer confirms database insertion. Ensure this directory is stored on persistent storage.

---

## 8. Upgrades & Maintenance

bunfiiliate migrations are non-destructive and backward-compatible.

### Upgrading with Docker Compose

```bash
cd /opt/bunfiiliate
git pull origin staging

# Rebuild and restart services
docker compose --env-file .env -f deploy/docker-compose.yml up -d --build
```

When `api` restarts, `bf_db::migrate(&pool)` runs automatically, ensuring the database schema matches the updated application version before traffic resumes.

### Graceful Shutdown

Both `api` and `tracker` listen for `SIGINT` and `SIGTERM`. When stopping:

- The tracker closes HTTP listeners, allows the writer queue to drain, and flushes pending spool records.
- The worker finishes processing current savepoint transactions before terminating.
