# bench

Reproducible load benchmark for the tracker ingress (`/c`, `/r/:code`, `/v1/events/:key`).

```bash
bench/run.sh                 # full run -> bench/results/<date>-<sha>.json
bench/run.sh --quick         # 2 s x 1 repeat, ->  ...-quick.json (never published)
bench/run.sh --write-docs    # full run, then regenerate docs/benchmarks.md + site datasheet
python -I bench/compare.py bench/results/X.json bench/baseline/<class>.json   # exit 0/1/3
python -I bench/compare.py --self-test
bench/tests/test_run.sh      # helper + quick-run tests (needs Postgres; BENCH_TEST_SKIP_QUICK=1 skips the run)
```

Requires `cargo psql openssl jq curl python3` and `oha` **1.16.0** (pinned; `BF_BENCH_ALLOW_OHA=1`
overrides, results then are not comparable). Missing `oha` exits 2 with an install hint; nothing is
installed for you.

| Env | Default | Meaning |
| --- | --- | --- |
| `DATABASE_URL` | `postgres://$USER@localhost:5433/postgres` | admin URL; scratch DB `bf_bench` is created and dropped |
| `BENCH_DURATION` | 10 | seconds per run (`10s` also accepted) |
| `BENCH_REPEATS` | 5 | runs per endpoint x fsync mode |
| `BENCH_CONC` | 20 | oha connections |
| `BENCH_WARMUP` | 3 | seconds, discarded |
| `BENCH_SETTLE` | 5 | seconds idle before every run so writer queue/ingest drain (else later runs hit 503 backpressure) |
| `BENCH_SEED` | 42 | seeds the referral code set and the per-run code choice |
| `BENCH_CLASS` | `<os>-<arch>-<cores>c` | hardware class key for baselines |
| `BENCH_SPOOL_ROOT` | `$TMPDIR/bf_bench_spool` | dedicated spool parent dir; its fs type is recorded |

The tracker runs with spool disk thresholds relaxed (as in e2e) so a busy dev disk does not shed.

## Result schema (`schema_version` 1)

```json
{
  "schema_version": 1, "date": "YYYY-MM-DD", "git_sha": "...", "quick": false,
  "hardware": {"cpu": "", "cores": 0, "ram_gb": 0, "os": "", "pg_version": "", "rustc": "",
               "oha_version": "", "git_sha": "", "fsync_modes": ["always","off"],
               "spool_fs": "", "class": ""},
  "params": {"duration_s": 10, "repeats": 5, "concurrency": 20, "warmup_s": 3, "seed": 42},
  "results": [{"endpoint": "/c", "fsync": "always",
               "runs": [{"rps": 0, "p50_ms": 0, "p95_ms": 0, "p99_ms": 0, "errors": 0, "status": {"204": 0}}],
               "median_rps": 0, "min_rps": 0, "max_rps": 0, "median_p50_ms": 0, "median_p95_ms": 0,
               "median_p99_ms": 0, "max_p99_ms": 0, "errors_total": 0}]
}
```

`errors` counts transport errors plus any response outside 2xx/3xx. Any non-zero count fails the run.

## Regression rules

`thresholds.json`: fail if median rps < baseline x 0.80 or median p99 > baseline x 1.5; errors > 0
always fails. Baselines are per hardware class in `bench/baseline/`. The nightly workflow
(`.github/workflows/bench-nightly.yml`) is advisory on GitHub-hosted runners (noisy) and
authoritative on a self-hosted runner labelled `bench` (enable with repo variable
`BENCH_SELF_HOSTED=true`). Publishing rules: see `docs/benchmarks.md`.
