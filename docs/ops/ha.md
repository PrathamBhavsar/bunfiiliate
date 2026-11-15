# High availability notes

bunfiiliate is built and tested as a **single node**. This page says what that gives you, what you can add yourself, and what is not supported.

## Single node baseline

| | Value | Why |
|---|---|---|
| RPO, Postgres | up to 24 h (nightly dump); minutes with WAL archiving | `docs/ops/backup-restore.md` |
| RPO, clicks/events in flight | 0 for acknowledged records | spool is fsynced before the answer; replayed after a crash |
| RTO | about 15-60 min | provision, `restore.sh`, start; depends on dump size |

## Optional: Postgres streaming replica

Standby on another host: `pg_basebackup -h primary -U replicator -D /var/lib/postgresql/16/main -R -X stream` (`-R` writes `primary_conninfo` and `standby.signal`). The primary needs a replication role, `wal_level = replica`, an entry in `pg_hba.conf`. **Promotion is manual**: `pg_ctl promote` (or `SELECT pg_promote()`), then point `DATABASE_URL` at it and restart the services. Nothing here automates failover; replication is asynchronous, so promoting can lose the last seconds of writes.

Reads on a replica for analytics (`BF_DATABASE_URL_RO`) is a **design note only: not implemented**. All services use `DATABASE_URL`.

## Two trackers

The tracker keeps no state except its spool and an in-memory v2 idempotency map. Run two behind Caddy (`reverse_proxy tracker1:8081 tracker2:8081`, health checks on `/readyz`):

- **Separate spool directories**, one per tracker (never share one: the spool format has a single writer).
- Both write to the same Postgres. Click ids are primary keys with `ON CONFLICT DO NOTHING`, so a record replayed or sent twice does not duplicate.
- The v2 duplicate-event check (`Idempotency-Key`) is per process memory: a retry that lands on the other tracker is not seen as a duplicate by it. The event is still stored once per external id downstream (`inbound_events` dedupe), but the first-event id in the duplicate answer can differ.
- A tracker that dies keeps its spool; start it again (or mount the disk elsewhere) and it replays.

## Api and worker

`api serve --with-worker` and `api worker` can run on more than one node. What protects against double work, verified in `crates/api/src/worker.rs`, `dispatch.rs`:

- approve and rollup ticks take `pg_try_advisory_xact_lock`, so one node does a tick at a time (the others skip it);
- the outbox, webhook deliveries and inbound events are claimed with `FOR UPDATE SKIP LOCKED`.

There is **no single leader election**: the nodes all run the loops and the locks keep them from clashing. Two api nodes behind Caddy need only the shared `JWT` secret and master keys; the version cache is per node (revocation bound 5 s, see `docs/plan/REVISIONS.md` A08) and the rate limiter is per node (limits multiply by node count).

## Failure matrix

| Failure | Effect | Recovery |
|---|---|---|
| Tracker (one of two) down | Caddy routes to the other; no loss | restart |
| Tracker (single) down | clicks and events get no answer; providers retry webhooks | restart; its spool replays |
| Spool disk full | 503 on `/c` and events, `/r` redirects without recording | `docs/ops/spool-disk.md` |
| Postgres down | tracker keeps spooling until the disk fills; api returns errors | restore service; spool drains |
| Postgres host lost | no writes | promote the replica, or `restore.sh` from the last backup; replay the spool |
| Api/worker down | no admin UI, no approvals, no webhook dispatch; tracking continues | restart; the backlog drains |
| Caddy down | everything is unreachable | restart |

## Not supported

- Multi-region or active-active writes.
- Automatic failover of Postgres, Caddy or any service (use your platform's supervisor; failover here is manual).
- Sharing one spool directory between trackers.
- Read replicas in the application (`BF_DATABASE_URL_RO` is not implemented).
