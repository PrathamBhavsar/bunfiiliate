# Baselines

One file per hardware class: `bench/baseline/<hardware.class>.json`, in the same schema as a
`bench/results/*.json` full run. `compare.py` exits 3 when the file for the run's class is
missing or its `hardware.class` differs.

Updating a baseline requires a PR that touches `bench/baseline/*` and gives the reason in the
description (hardware change, intended perf change, runner image bump). Do not refresh a
baseline to make a regression pass.

No baseline is committed yet: create `bench-selfhosted.json` from the first full run on the
self-hosted runner (`cp bench/results/<date>-<sha>.json bench/baseline/bench-selfhosted.json`).
