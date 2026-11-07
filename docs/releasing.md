# Releasing

## Cut release

1. Bump `version` in `[workspace.package]` of `Cargo.toml`. `cargo check` to refresh `Cargo.lock`. Merge via staging -> prod.
2. Tag `vX.Y.Z` (must equal Cargo version, dist enforces). On prod.
3. `git push origin vX.Y.Z`. Done. Three workflows fire on the tag:
   - `release.yml` (cargo-dist): builds bins, makes GitHub Release.
   - `web-asset.yml`: builds `apps/web`, uploads tarball to that release (waits up to 45 min for release).
   - `docker.yml`: multi-arch image to GHCR.

Check before tagging: `dist plan` and `dist generate --check` (dist 0.34.0, `brew install cargo-dist`). After editing `dist-workspace.toml`, run `dist generate` and commit `release.yml`.

## Assets (release vX.Y.Z)

Per package `api` and `tracker`, each target `T` in
`aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`:

- `api-T.tar.xz`, `api-T.tar.xz.sha256`
- `tracker-T.tar.xz`, `tracker-T.tar.xz.sha256`

Plus:

- `api-installer.sh`, `tracker-installer.sh` (shell installers)
- `source.tar.gz`, `source.tar.gz.sha256`
- `sha256.sum`
- `dist-manifest.json`
- `bunfiiliate-web-static.tar.gz`, `bunfiiliate-web-static.tar.gz.sha256` (from web-asset.yml; contents = `apps/web/out/`)

Docker: `ghcr.io/prathambhavsar/bunfiiliate:{X.Y.Z,X.Y,latest}` (linux/amd64, linux/arm64).

## Install

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/PrathamBhavsar/bunfiiliate/releases/latest/download/api-installer.sh | sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/PrathamBhavsar/bunfiiliate/releases/latest/download/tracker-installer.sh | sh
```

Installs to `$CARGO_HOME/bin` (`~/.cargo/bin`). Pin a version: replace `latest/download` with `download/vX.Y.Z`.

## Notes

- No sqlx compile-time macros (`query!`); `migrate!` reads files only. No DB or `SQLX_OFFLINE` needed at build.
- Linux builds install `libssl-dev pkg-config` (native-tls). Linux x86_64 built on ubuntu-22.04 (glibc 2.35); aarch64 on ubuntu-22.04-arm.
- Web build uses pnpm (repo lockfile is pnpm), same as ci.yml.
