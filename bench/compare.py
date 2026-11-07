#!/usr/bin/env python3
"""Compare a bench result against a baseline (stdlib only; run with `python -I`).

  python -I bench/compare.py RESULT.json BASELINE.json [--thresholds T.json] [--noisy]
  python -I bench/compare.py --self-test

Exit: 0 pass (or warnings only on a noisy runner), 1 regression/errors, 2 usage/bad input,
3 baseline missing or for a different hardware class.

Rules (bench/thresholds.json): fail if median rps < baseline * rps_min_ratio, or median p99 >
baseline * p99_max_ratio. Any error count > 0 always fails, even with --noisy.
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULTS = {"rps_min_ratio": 0.80, "p99_max_ratio": 1.50, "errors_max": 0}


def load(path):
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def index(doc):
    return {(r["endpoint"], r["fsync"]): r for r in doc["results"]}


def evaluate(result, baseline, thresholds=None, noisy=False):
    """Return (exit_code, failures, warnings, notes)."""
    th = dict(DEFAULTS)
    th.update(thresholds or {})
    failures, warnings, notes = [], [], []

    rclass = result.get("hardware", {}).get("class")
    bclass = baseline.get("hardware", {}).get("class")
    if not bclass or rclass != bclass:
        return 3, [f"baseline class {bclass!r} does not match result class {rclass!r}"], [], []

    base = index(baseline)
    for key, cur in index(result).items():
        label = f"{key[0]} [fsync={key[1]}]"
        if cur.get("errors_total", 0) > th["errors_max"]:
            failures.append(f"{label}: {cur['errors_total']} request errors (always fatal)")
        b = base.get(key)
        if b is None:
            notes.append(f"{label}: not in baseline, skipped")
            continue
        soft = []
        if cur["median_rps"] < b["median_rps"] * th["rps_min_ratio"]:
            soft.append(
                f"{label}: median rps {cur['median_rps']} < {th['rps_min_ratio']:.2f} x baseline {b['median_rps']}"
            )
        if cur["median_p99_ms"] > b["median_p99_ms"] * th["p99_max_ratio"]:
            soft.append(
                f"{label}: median p99 {cur['median_p99_ms']} ms > {th['p99_max_ratio']:.2f} x baseline {b['median_p99_ms']} ms"
            )
        (warnings if noisy else failures).extend(soft)
        if not soft:
            notes.append(f"{label}: ok (rps {cur['median_rps']}, p99 {cur['median_p99_ms']} ms)")
    for key in base:
        if key not in index(result):
            failures.append(f"{key[0]} [fsync={key[1]}]: missing from result")
    return (1 if failures else 0), failures, warnings, notes


def self_test():
    import importlib.util
    import unittest

    path = os.path.join(HERE, "tests", "test_compare.py")
    if not os.path.exists(path):
        print(f"self-test file missing: {path}", file=sys.stderr)
        return 2
    spec = importlib.util.spec_from_file_location("test_compare", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    suite = unittest.defaultTestLoader.loadTestsFromModule(mod)
    res = unittest.TextTestRunner(verbosity=1).run(suite)
    return 0 if res.wasSuccessful() else 1


def main(argv):
    if "--self-test" in argv:
        return self_test()
    noisy = "--noisy" in argv or os.environ.get("BENCH_NOISY") == "1"
    args = [a for a in argv if a != "--noisy"]
    tpath = os.path.join(HERE, "thresholds.json")
    if "--thresholds" in args:
        i = args.index("--thresholds")
        tpath = args[i + 1]
        del args[i : i + 2]
    if len(args) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    rpath, bpath = args
    if not os.path.exists(bpath):
        print(f"baseline missing: {bpath}", file=sys.stderr)
        return 3
    try:
        result, baseline = load(rpath), load(bpath)
        thresholds = load(tpath) if os.path.exists(tpath) else {}
    except (OSError, ValueError) as e:
        print(f"cannot read input: {e}", file=sys.stderr)
        return 2
    code, failures, warnings, notes = evaluate(result, baseline, thresholds, noisy)
    for n in notes:
        print("ok   ", n)
    for w in warnings:
        print("WARN ", w + " (noisy runner: advisory)")
    for f in failures:
        print("FAIL ", f)
    print({0: "PASS", 1: "FAIL", 3: "NO BASELINE"}[code])
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
