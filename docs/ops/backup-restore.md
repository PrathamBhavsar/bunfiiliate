# Backup and restore

Scripts live in `deploy/backup/`. They need `pg_dump`, `pg_restore`, `psql` (set `PG_BIN` to a directory holding PG16+ tools when the default ones are older than the server) and `tar`.

## What is backed up

| Data | How | Where |
|---|---|---|
| Postgres (config, users, conversions, ledger) | `pg_dump -Fc` | `$BF_BACKUP_DIR/pg/<ts>.dump` + `.sha256` |
| Spool (clicks and events not yet in Postgres) | copy of `checkpoint` and `*.wal`, tarred | `$BF_BACKUP_DIR/spool/<ts>.tar` + `.sha256` |
| Master keys (`BF_MASTER_KEYS`) | **not** by these scripts; store them elsewhere | see `docs/self-hosting.md` |

`backup.sh` keeps the newest `BF_BACKUP_KEEP` (default 14) of each. `BF_BACKUP_REMOTE` (optional) is copied to with `rclone copy`, else `rsync -a`.

## Order (why)

1. Pause spool GC by creating `$SPOOL_DIR/gc.paused`. The tracker's GC then deletes nothing, so no segment vanishes mid-copy. (Anything else that calls `bf_spool::delete_segments_below` honours the flag.)
2. Copy the `checkpoint` file first, then the segments. The copied checkpoint can only be at or behind the segments' content.
3. Run `pg_dump`. The database may be ahead of the copied checkpoint. That is safe: restore replays the spool from the checkpoint and inserts are idempotent (click id primary key, `ON CONFLICT DO NOTHING`; `crates/spool/tests/restore_replay.rs`).
4. Resume GC (removed by a trap, also when the backup fails). If a backup is killed with `kill -9` the flag stays: delete `$SPOOL_DIR/gc.paused` by hand, otherwise the spool grows (the `SpoolDiskLow` alert will tell you).

The active segment is copied while the tracker writes to it, so its tail may be torn. Replay stops at a torn record, as after a crash.

## Schedule

`deploy/systemd/bf-backup.{service,timer}`: nightly 02:30 (+ up to 15 min jitter). Install the scripts to `/usr/local/share/bunfiiliate/backup/`, create `/var/backups/bunfiiliate`, `systemctl enable --now bf-backup.timer`.

## Restore

```bash
deploy/backup/restore.sh --dump $BF_BACKUP_DIR/pg/<ts>.dump \
  --spool-tar $BF_BACKUP_DIR/spool/<ts>.tar \
  --target-url postgres://.../newdb --spool-target /var/spool/bunfiiliate
```

- Checksums are verified first; a mismatch or a missing `.sha256` aborts before anything is touched.
- A non-empty target database or spool dir is refused. `--force` restores with `pg_restore --clean --if-exists` and moves an existing spool dir aside to `<dir>.pre-restore.<unix-ts>` (never deletes it).
- Start the tracker afterwards: it replays the spool from the restored checkpoint into Postgres before serving.

## Point-in-time recovery (optional)

Not set up by the scripts. For PITR enable WAL archiving on Postgres: `archive_mode = on`, `archive_command = 'test ! -f $BF_BACKUP_DIR/wal/%f && cp %p $BF_BACKUP_DIR/wal/%f'`, take base backups with `pg_basebackup -Ft -z`, and follow the Postgres documentation for `recovery_target_time`. The nightly dump remains the baseline.

## Drill

`make backup-drill` runs `scripts/tests/backup_drill.sh` against `DATABASE_URL` (it creates and drops its own databases): seeds a balanced ledger, backs up, checks checksum-mismatch and non-empty-target refusal, restores and compares row counts and an md5 of the ledger, checks retention, then runs `deploy/backup/drill.sh`.

`drill.sh` alone runs against a live install (`DATABASE_URL`, `SPOOL_DIR`): it takes a backup, restores it into a scratch database and temp spool dir, and compares `affiliates`, `conversions`, `payouts`, `ledger_entries` counts, the ledger hash and the count of unbalanced transactions (`SUM(amount_cents) <> 0` per `txn_id`, must be 0). It prints `PASS` or `FAIL` and exits non-zero on a mismatch. Run it after every upgrade and at least monthly.

Limits: the drill does not start a tracker to replay the restored spool; replay idempotency is covered by the Rust test. The drill fingerprint includes `clicks_total` and one `clicks:<partition>=<n>` line per partition (C24), so a lost or mis-routed partition fails the drill; test `drill_counts_per_partition` in `scripts/tests/backup_drill.sh`. A partition that retention detached or dropped after a dump was taken is not in that dump's later state; a restored old dump brings those rows back, so re-run retention after a restore. `restore.sh --force` wipes the `public` schema before restoring (`pg_restore --clean` cannot drop inherited partition keys).
