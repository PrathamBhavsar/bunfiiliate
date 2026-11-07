# Load Benchmarks

Empirical load test results for `tracker` ingress endpoints under continuous concurrency.

## Test Environment

> Legacy run (pre-C40 harness, single 5 s run per cell, no git sha recorded). The next clean full `bench/run.sh --write-docs` run replaces this file with a schema v1 result including git sha, hardware class and min/max across repeats.

- **CPU**: Apple M4 (10 physical/virtual cores)
- **RAM**: 16 GB
- **Operating System**: macOS 26.5.1
- **PostgreSQL**: PostgreSQL 16.15 (Homebrew) on aarch64-apple-darwin25.6.0
- **Rust Toolchain**: cargo 1.99.0 (5f94df478 2026-08-27)
- **Load Generator**: oha 1.16.0
- **Parameters**: Concurrency = 20 connections, Duration = 5s per endpoint

> [!NOTE]
> **Single-Machine Localhost Run**: All tests ran on a single development machine. The load generator (`oha`), the `tracker` binary, and PostgreSQL shared the same CPU cores, RAM, and SSD I/O bandwidth. Treat these numbers as a floor for this machine, not a production capacity claim.

## Results Summary

| Endpoint | `BF_FSYNC` | Requests/sec | Latency p50 | Latency p95 | Latency p99 | Errors | Status Codes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `/c` | always | 1455.74 | 13.942 ms | 18.848 ms | 23.123 ms | 0 | 204: 7301 |
| `/r/:code` | always | 1528.31 | 13.107 ms | 17.168 ms | 19.788 ms | 0 | 302: 7663 |
| `/v1/events/:key` | always | 1548.84 | 12.857 ms | 17.228 ms | 22.878 ms | 0 | 202: 7760 |
| `/c` | off | 16993.22 | 0.645 ms | 4.203 ms | 4.78 ms | 0 | 204: 84993 |
| `/r/:code` | off | 27591.99 | 0.405 ms | 3.695 ms | 4.33 ms | 0 | 302: 138041 |
| `/v1/events/:key` | off | 9039.3 | 1.377 ms | 4.844 ms | 5.449 ms | 0 | 202: 45241 |

## Architecture & Analysis

1. **Hot-Path Validation**: Every request is validated against an in-memory snapshot (`Snapshot` in `ArcSwap`). There are zero synchronous database round-trips during request handling.
2. **Durability Modes**:
   - **`BF_FSYNC=always`**: The spool writer groups requests and issues an `fdatasync`/`fsync` per group before acknowledging the HTTP request. Latency is dominated by the flush wait (about 13 ms p50 in this run).
   - **`BF_FSYNC=off`**: Spool writes enter the OS page cache immediately. Records are still written to the segmented WAL but not flushed per batch, so a host crash can lose recent clicks. In this run that gave roughly 6x to 18x the `always` throughput (9k to 28k req/sec) with sub-millisecond p50.
3. **Decoupled Postgres Ingest**: A background worker batch-inserts spooled records into Postgres using `UNNEST` and advances the WAL checkpoint asynchronously, shielding ingress from database lock contention.

## Publishing Rules

These rules apply to every benchmark number in this repo, the README, and `site/index.html`.

1. Numbers are published only from a full `bench/run.sh` run whose result JSON (with its `hardware` block and git sha) is committed under `bench/results/`. Quick runs (`--quick`) are never published.
2. Always state the hardware, the `BF_FSYNC` mode, and the localhost single-machine caveat (the NOTE above).
3. Never mix `BF_FSYNC` modes in one claim. Quote `always` and `off` separately.
4. These numbers are a floor for the measured machine, not a capacity claim.
5. No comparison claims against other products without a run of both on the same hardware.
6. Regenerate with `bench/run.sh --write-docs`. It rewrites this file and the datasheet in `site/index.html`. `scripts/check-bench-numbers.sh` (part of the final gate) fails if the site drifts from the latest result.
7. Regression gating: `python -I bench/compare.py RESULT BASELINE` against `bench/baseline/<hardware class>.json`. Updating a baseline requires a PR that touches `bench/baseline/*` and states the reason in the description.
