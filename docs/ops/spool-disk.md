# Spool disk full

The tracker writes every click and event to the spool before it answers. If the spool disk fills, writes fail, so the tracker sheds load on purpose instead of losing records it already acknowledged.

## Config (tracker env)

| Var | Default | Meaning |
|---|---|---|
| `BF_SPOOL_HIGH_WATER_PCT` | 85 | start shedding at this percent used |
| `BF_SPOOL_LOW_WATER_PCT` | 70 | stop shedding below this (never above the high mark) |
| `BF_SPOOL_MIN_FREE_BYTES` | 268435456 | also shed under this many free bytes |
| `BF_SPOOL_MAX_BYTES` | unset | optional cap on total segment bytes |

A background task samples the filesystem (`statvfs`) every second. The two marks give hysteresis, so it does not flap. On the step into shedding it runs a GC below the checkpoint to free segments (not while `gc.paused` exists, see `backup-restore.md`). A write that fails with ENOSPC also starts shedding at once, the batch is rolled back to its last good length and the caller gets `SpoolError::Full`: no partial record is left.

## Per-route behaviour while shedding

| Route | Response |
|---|---|
| `POST /c`, `POST /v2/c` (clicks) | **503** + `Retry-After: 5` |
| `GET /r/{code}` | still **302** to the landing page, **but no click is recorded** |
| `POST /v1/events/{key}`, `/v2/events/{key}` | **503** + `Retry-After: 5` |
| Stripe / Paddle webhooks (v1 and v2) | **503** + `Retry-After: 5` (providers retry) |

`/r/{code}` is the one lossy route: the visitor reaches the page, so a dead link never costs a sale, but that click is gone, and the redirect URL carries a `bf_click` id that has no row. A conversion later attributed to it will not find a click. Every dropped redirect increments `bf_spool_shed_total{route="r"}`. Events (money) and beacon clicks are never dropped silently: the caller sees the 503 and can retry.

## Metrics

`bf_spool_free_bytes`, `bf_spool_disk_bytes`, `bf_spool_shedding` (0/1), `bf_spool_shed_total{route="c|r|events"}`, plus the existing `bf_spool_bytes` and `bf_spool_lag`. Alerts: `SpoolShedding`, `SpoolDiskLow`, `SpoolLagHigh` (`docs/ops/alerts.md`).

## Runbook

1. `bf_spool_lag` high: Postgres is the bottleneck or down. Fix it; the checkpoint advances and GC frees segments by itself.
2. Lag is low but the disk is full: something else uses the disk, or `gc.paused` was left behind by a killed backup (`rm $SPOOL_DIR/gc.paused`).
3. Give the spool a bigger or dedicated volume. Shedding clears on its own below the low-water mark.

## Tests

`cargo test -p bf-spool --test disk_full`, `cargo test -p tracker --test shedding`. The tiny-tmpfs end-to-end test is Linux-only and ignored by default: `scripts/tmpfs-test.sh` (root, e.g. `docker run --privileged`); on macOS it is skipped.
