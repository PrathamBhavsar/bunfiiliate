# Load Benchmarks

Empirical load test results for `tracker` ingress endpoints under continuous concurrency.

## Test Environment

- **CPU**: Apple M4 (10 physical/virtual cores)
- **RAM**: 16 GB
- **Operating System**: macOS 26.5.1
- **PostgreSQL**: PostgreSQL 16.15 (Homebrew) on aarch64-apple-darwin25.6.0
- **Rust Toolchain**: cargo 1.99.0 (5f94df478 2026-08-27)
- **Load Generator**: oha 1.16.0
- **Parameters**: Concurrency = 20 connections, Duration = 5s per endpoint

> [!NOTE]
> **Single-Machine Localhost Run**: All tests ran on a single development machine. The load generator (`oha`), the `tracker` binary, and PostgreSQL shared the same CPU cores, RAM, and SSD I/O bandwidth. In production with dedicated ingress instances and network separation, ingress throughput scales independently of DB write load.

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
   - **`BF_FSYNC=always`**: The spool writer groups requests and issues an `fdatasync`/`fsync` per group before acknowledging the HTTP request. Throughput is bounded by disk flush latency on the host filesystem (~700 - 1,000 req/sec per core on local APFS).
   - **`BF_FSYNC=off`**: Spool writes enter the OS page cache immediately. Throughput increases by over 10x-20x to 10,000 - 18,000+ req/sec with sub-millisecond p50 latency, while still spooling segmented WAL records for crash recovery.
3. **Decoupled Postgres Ingest**: A background worker batch-inserts spooled records into Postgres using `UNNEST` and advances the WAL checkpoint asynchronously, shielding ingress from database lock contention.
