---
type: Lesson
title: Bench
description: Lessons from the C40-C42 benchmark work.
tags: [wisdom, bench]
---

# Bench

- Nested heredocs: a script written with `cat <<'EOF'` that itself contains `<< EOF` ends early and runs the tail as shell. Use unique delimiters (`BENCH_RUN_END`).
- Benchmarks on a disk above 85% used trip spool shedding (503s): the bench relaxes the thresholds like e2e does.
- Count non-2xx/3xx responses as errors, not just oha transport errors.
- Publish only full runs; quick runs write `*-quick.json` and are ignored by the number check.
- Finding: four full runs in a row showed 503 backpressure on `/r/:code` with `BF_FSYNC=off` (one 10 s run of five, 13k to 165k shed requests at 28k to 40k rps), even with 10 s settle between runs. The tracker writer queue sheds at saturation. The error gate correctly refused to publish, so the committed numbers are still the legacy run. Follow-up: investigate `/r` shedding at that rate, or lower `BENCH_CONC`, then run `bench/run.sh --write-docs`.
