---
title: Install
group: Start
order: 2
description: Release binaries, Docker Compose, or build from source. Verify checksums.
---

# Install

Pick one. All of them need PostgreSQL 16.

## Release binaries

Releases publish `api` and `tracker` for macOS and Linux, x86_64 and arm64.

```bash
curl -LsSf https://github.com/PrathamBhavsar/bunfiiliate/releases/latest/download/api-installer.sh | sh
curl -LsSf https://github.com/PrathamBhavsar/bunfiiliate/releases/latest/download/tracker-installer.sh | sh
```

> No release is published yet. Until the first tag, use Docker or build from source.

### Verify a download

Each archive ships with a `.sha256` file, and the release carries `sha256.sum`.

```bash
sha256sum -c sha256.sum --ignore-missing
```

On macOS use `shasum -a 256 -c sha256.sum --ignore-missing`.

## Docker Compose

The recommended path for a VPS. See [Deploy](#/deploy) for the full walkthrough.

```bash
git clone https://github.com/PrathamBhavsar/bunfiiliate && cd bunfiiliate
cp .env.example .env
docker compose --env-file .env -f deploy/docker-compose.yml up -d
```

A multi-arch image is published to `ghcr.io/prathambhavsar/bunfiiliate` on each release.

## From source

Needs Rust 1.85+ and Node 20+ (to minify `tracker.js`).

```bash
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH   # macOS, brew rustup
cargo build --release -p api -p tracker
```

The release web UI is a static export of `apps/web`, also attached to each release as `bunfiiliate-web-static.tar.gz`.
