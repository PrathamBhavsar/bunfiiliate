# syntax=docker/dockerfile:1

# ------------------------------------------------------------------------------
# Stage 1: Build Rust release binaries (api and tracker)
# ------------------------------------------------------------------------------
FROM rust:1-slim-bookworm AS builder-rust

WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/

RUN cargo build --release --bin api --bin tracker

# ------------------------------------------------------------------------------
# Stage 2: Build static frontend export (apps/web out)
# ------------------------------------------------------------------------------
FROM node:20-bookworm-slim AS builder-web

WORKDIR /build

RUN corepack enable && corepack prepare pnpm@latest --activate

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY sdks/ ./sdks/
COPY apps/web/ ./apps/web/

RUN pnpm install --frozen-lockfile
RUN pnpm -C apps/web build

# ------------------------------------------------------------------------------
# Stage 3: Minimal production runtime (one image, both binaries + static assets)
# ------------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -g 10001 bunfiiliate && \
    useradd -u 10001 -g bunfiiliate -s /bin/sh -d /var/lib/bunfiiliate bunfiiliate

RUN mkdir -p /var/spool/bunfiiliate /srv/web /var/lib/bunfiiliate /etc/bunfiiliate && \
    chown -R bunfiiliate:bunfiiliate /var/spool/bunfiiliate /srv/web /var/lib/bunfiiliate

COPY --from=builder-rust /build/target/release/api /usr/local/bin/api
COPY --from=builder-rust /build/target/release/tracker /usr/local/bin/tracker
COPY --from=builder-web /build/apps/web/out /srv/web
COPY deploy/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENV API_PORT=8080 \
    TRACKER_PORT=8081 \
    SPOOL_DIR=/var/spool/bunfiiliate

EXPOSE 8080 8081

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["api", "serve", "--with-worker"]
