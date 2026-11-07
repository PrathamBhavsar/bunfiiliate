# Clicks partitioning (C20-C22)

`clicks` is range-partitioned by month on `ts` (UTC bounds), migration `0080`. Primary key is `(id, ts)`.
The click insert uses bare `ON CONFLICT DO NOTHING`; `ts` is derived from the spooled record, so a
replay hits the same `(id, ts)` and dedups.

## Partitions
- Named `clicks_yYYYYmMM`. `clicks_default` is the catch-all.
- `bf_ensure_clicks_partition(month)` (SQL) creates one idempotently: it creates a standalone table, moves
  that month's rows out of `clicks_default`, then attaches (attaching over default rows would fail).
  Serialised by an advisory lock; inserts to default are blocked briefly (SHARE ROW EXCLUSIVE).
- The api calls `ensure_partitions(months_ahead=3)` at start; the worker repeats it every 6 h.
- Rows in `clicks_default` mean a month is missing. Gauge `bf_clicks_default_rows` (SQL fn `bf_clicks_default_rows()`);
  alert on `> 0` (C12 rules).

## Migration
Old indexes/constraints are renamed `clicks_old_*` before the new ones are created. Data is copied one
month at a time, oldest first, but inside the migration transaction (sqlx). For a very large table, run
the upgrade in a maintenance window, or pre-copy using the same month-by-month `INSERT ... SELECT`
into the new table before cutting over. No FK references `clicks(id)`.

## Retention
`BF_CLICKS_RETENTION_MONTHS` (default `0` = keep forever). Daily-ish (every 6 h) the worker drops whole
month partitions that ended before `now - months` and before `now - 7 days` (retention must be >= the
rollup window; the rollup rebuilds only the last day, so 7 days is a hard floor).

A partition is dropped only if every program has a `rollup_state` row and the oldest `ran_at` is at
least one day past the partition end. Otherwise it is kept and an `audit_log` row
`clicks.partition_skipped` records the reason. Drops write `clicks.partition_dropped`.

`daily_rollups` is never touched. The dropped boundary is stored in `clicks_retention.floor_day`; the
rollup never rebuilds days before it (it would count zero clicks and erase the rows), even for a
back-dated conversion. Detached/dropped partitions are not in older dumps' later state; restoring an
old dump brings the rows back as default/partition rows, so re-run retention after a restore.

## Pruning (C21)
Queries on `clicks` carry a `ts` bound where one exists: the rollup click leg
(`COALESCE($2, '-infinity')` stable lower bound; guarded by `clicks_queries_prune_partitions`),
the billing usage count, and the attribution touch window. Id-only lookups
(`first_click_id` joins, the touch seed click) cannot prune: they probe the PK index of each partition.
The per-code stats query (C30) must add its own `ts` test; deferred until C30 lands.
