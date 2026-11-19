#!/usr/bin/env python3
"""Render docs/benchmarks.md and the benchmark facts in site/src/brand.js from a full bench result.

  python -I bench/gen_docs.py RESULT.json          # write docs + site facts
  python -I bench/gen_docs.py --check RESULT.json  # verify docs + facts match the result (exit 1 on drift)
  python -I bench/gen_docs.py --latest             # print path of latest full result, if any
"""
import glob
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BRAND = os.path.join(ROOT, "site", "src", "brand.js")
DOCS = os.path.join(ROOT, "docs", "benchmarks.md")

# (row id, endpoint, fsync) -- row ids are the data-benchmark keys in the docs table.
ROWS = [
    ("fsync_always_c", "/c", "always"),
    ("fsync_always_r", "/r/:code", "always"),
    ("fsync_always_webhook", "/v1/events/:key", "always"),
    ("fsync_off_c", "/c", "off"),
    ("fsync_off_r", "/r/:code", "off"),
    ("fsync_off_webhook", "/v1/events/:key", "off"),
]

# (fact row label in site/src/brand.js, BF_FSYNC mode) -- both quote `/r/:code`.
FACTS = [
    ("Tracker p50, fsync off", "off"),
    ("Tracker p50, fsync always", "always"),
]


def num(x):
    x = round(float(x), 3)
    s = repr(x)
    return s[:-2] if s.endswith(".0") else s


def rps(x):
    s = repr(round(float(x), 2))
    return s[:-2] if s.endswith(".0") else s


def ms(x):
    return f"{num(x)} ms"


def status_str(ep):
    tot = {}
    for r in ep["runs"]:
        for k, v in r.get("status", {}).items():
            tot[k] = tot.get(k, 0) + v
    return ", ".join(f"{k}: {v}" for k, v in sorted(tot.items())) or "N/A"


def values(doc):
    by = {(r["endpoint"], r["fsync"]): r for r in doc["results"]}
    out = {}
    for rid, ep, fs in ROWS:
        r = by[(ep, fs)]
        out[rid] = {
            "rps": rps(r["median_rps"]),
            "p50": ms(r["median_p50_ms"]),
            "p95": ms(r["median_p95_ms"]),
            "p99": ms(r["median_p99_ms"]),
            "errors": str(r["errors_total"]),
            "status": status_str(r),
        }
    return out


def load(path):
    with open(path, "r", encoding="utf-8") as f:
        doc = json.load(f)
    if doc.get("schema_version") != 1 or doc.get("quick"):
        sys.exit(f"{path}: not a full schema_version=1 result")
    return doc


def latest():
    files = sorted(
        p for p in glob.glob(os.path.join(ROOT, "bench", "results", "*.json")) if not p.endswith("-quick.json")
    )
    for p in reversed(files):
        try:
            with open(p, encoding="utf-8") as f:
                if json.load(f).get("schema_version") == 1:
                    return p
        except ValueError:
            pass
    return None


def fmt_ms(v):
    """Same shape as scripts/check-bench-numbers.sh: `0.4` below a millisecond, whole ms above."""
    return f"{v:.1f}".rstrip("0").rstrip(".") if v < 1 else f"{round(v)}"


def fmt_rps(v):
    return f"{v / 1000:.1f}k"


def brand_values(doc):
    """The `/r/:code` p50 + rps strings quoted in site/src/brand.js, by fact row label."""
    by = {(r["endpoint"], r["fsync"]): r for r in doc["results"]}
    out = {}
    for label, fs in FACTS:
        r = by[("/r/:code", fs)]
        out[label] = f"{fmt_ms(r['median_p50_ms'])} ms · {fmt_rps(r['median_rps'])} req/s"
    return out


def render_brand(js, doc):
    vals = brand_values(doc)
    for label, want in vals.items():
        pat = r'(\["' + re.escape(label) + r'",\s*")([^"]*)(")'
        js, n = re.subn(pat, lambda m: m.group(1) + want + m.group(3), js)
        if n != 1:
            sys.exit(f"brand.js fact row {label!r} not found exactly once")
    p50 = {label: f"{fmt_ms(next(r for r in doc['results'] if r['endpoint'] == '/r/:code' and r['fsync'] == fs)['median_p50_ms'])}"
           for label, fs in FACTS}
    faq = [
        (r"(measured )([0-9.]+)( ms p50)", p50["Tracker p50, fsync off"]),
        (r"(p50 was about )([0-9.]+)( ms)", p50["Tracker p50, fsync always"]),
    ]
    for pat, want in faq:
        js, n = re.subn(pat, lambda m: m.group(1) + want + m.group(3), js)
        if n != 1:
            sys.exit(f"brand.js FAQ sentence {pat!r} not found exactly once")
    return js


def check_brand(doc, js):
    errs = []
    for label, want in brand_values(doc).items():
        m = re.search(r'\["' + re.escape(label) + r'",\s*"([^"]*)"\]', js)
        if not m:
            errs.append(f"fact row {label!r} not found in site/src/brand.js")
        elif m.group(1) != want:
            errs.append(f"{label}: brand.js has {m.group(1)!r}, result expects {want!r}")
    p50 = {fs: fmt_ms(next(r for r in doc["results"] if r["endpoint"] == "/r/:code" and r["fsync"] == fs)["median_p50_ms"]) for _, fs in FACTS}
    for pat, fs, what in (
        (r"measured ([0-9.]+) ms p50", "off", "FAQ fsync-off p50"),
        (r"p50 was about ([0-9.]+) ms", "always", "FAQ fsync-always p50"),
    ):
        m = re.search(pat, js)
        if not m:
            errs.append(f"{what} sentence not found in site/src/brand.js")
        elif m.group(1) != p50[fs]:
            errs.append(f"{what}: brand.js has {m.group(1)} ms, result expects {p50[fs]} ms")
    return errs


def doc_cells(line):
    return [c.strip() for c in line.strip().strip("|").split("|")]


def check_docs(doc, text):
    vals = values(doc)
    errs = []
    rows = {}
    for line in text.splitlines():
        cells = doc_cells(line)
        if len(cells) >= 4 and cells[0].startswith("`") and cells[1] in ("always", "off"):
            rows[(cells[0].strip("`"), cells[1])] = cells
    for rid, ep, fs in ROWS:
        cells = rows.get((ep, fs))
        if not cells:
            errs.append(f"docs/benchmarks.md has no `{ep}` / {fs} row")
            continue
        if cells[2].split()[0] != vals[rid]["rps"]:
            errs.append(f"[{ep} {fs}] rps: docs has {cells[2]!r}, result expects {vals[rid]['rps']}")
        if cells[3] != vals[rid]["p50"]:
            errs.append(f"[{ep} {fs}] p50: docs has {cells[3]!r}, result expects {vals[rid]['p50']}")
    return errs


def md(doc):
    hw, p = doc["hardware"], doc["params"]
    rows = []
    by = {(r["endpoint"], r["fsync"]): r for r in doc["results"]}
    for fs in ("always", "off"):
        for ep in ("/c", "/r/:code", "/v1/events/:key"):
            r = by[(ep, fs)]
            rows.append(
                f"| `{ep}` | {fs} | {rps(r['median_rps'])} ({rps(r['min_rps'])} to {rps(r['max_rps'])}) "
                f"| {ms(r['median_p50_ms'])} | {ms(r['median_p95_ms'])} | {ms(r['median_p99_ms'])} "
                f"| {r['errors_total']} | {status_str(r)} |"
            )
    table = "\n".join(rows)
    return f"""# Load Benchmarks

<!-- generated by `bench/run.sh --write-docs` from bench/results/{doc['date']}-{doc['git_sha']}.json; do not edit the generated section by hand -->

Load test results for the `tracker` ingress endpoints.

## Test Environment

- **CPU**: {hw['cpu']} ({hw['cores']} cores)
- **RAM**: {hw['ram_gb']} GB
- **Operating System**: {hw['os']}
- **PostgreSQL**: {hw['pg_version']}
- **Rust Toolchain**: {hw['rustc']}
- **Load Generator**: {hw['oha_version']}
- **Git SHA**: `{doc['git_sha']}`
- **Hardware class**: `{hw['class']}`
- **Spool filesystem**: {hw['spool_fs']}
- **`BF_FSYNC` modes**: {', '.join(hw['fsync_modes'])} (reported separately, never mixed)
- **Parameters**: {p['concurrency']} connections, {p['duration_s']} s per run, {p['repeats']} repeats per endpoint and mode, {p['warmup_s']} s discarded warm-up, seed {p['seed']}

> [!NOTE]
> **Single-Machine Localhost Run**: All tests ran on a single machine. The load generator (`oha`), the `tracker` binary, and PostgreSQL shared the same CPU cores, RAM, and SSD I/O bandwidth. Treat these numbers as a floor for this machine, not a production capacity claim.

## Results Summary

Requests/sec is the median of {p['repeats']} runs with (min to max). Latencies are medians across runs.

| Endpoint | `BF_FSYNC` | Requests/sec (min to max) | Latency p50 | Latency p95 | Latency p99 | Errors | Status Codes |
| --- | --- | --- | --- | --- | --- | --- | --- |
{table}

## Architecture & Analysis

1. **Hot-Path Validation**: Every request is validated against an in-memory snapshot (`Snapshot` in `ArcSwap`). There are zero synchronous database round-trips during request handling.
2. **Durability Modes**:
   - **`BF_FSYNC=always`**: The spool writer groups requests and issues an `fdatasync`/`fsync` per group before acknowledging the HTTP request. Latency is dominated by the flush wait.
   - **`BF_FSYNC=off`**: Spool writes enter the OS page cache immediately. A host crash can lose recent clicks. Throughput is much higher with sub-millisecond p50.
3. **Decoupled Postgres Ingest**: A background worker batch-inserts spooled records into Postgres using `UNNEST` and advances the WAL checkpoint asynchronously, shielding ingress from database lock contention.

{RULES}"""


RULES = """## Publishing Rules

These rules apply to every benchmark number in this repo, the README, and the site copy (`site/src/brand.js`).

1. Numbers are published only from a full `bench/run.sh` run whose result JSON (with its `hardware` block and git sha) is committed under `bench/results/`. Quick runs (`--quick`) are never published.
2. Always state the hardware, the `BF_FSYNC` mode, and the localhost single-machine caveat (the NOTE above).
3. Never mix `BF_FSYNC` modes in one claim. Quote `always` and `off` separately.
4. These numbers are a floor for the measured machine, not a capacity claim.
5. No comparison claims against other products without a run of both on the same hardware.
6. Regenerate with `bench/run.sh --write-docs`. It rewrites this file and the benchmark facts in `site/src/brand.js`. `scripts/check-bench-numbers.sh` (part of the final gate) fails if the site drifts from the latest result.
7. Regression gating: `python -I bench/compare.py RESULT BASELINE` against `bench/baseline/<hardware class>.json`. Updating a baseline requires a PR that touches `bench/baseline/*` and states the reason in the description.
"""


def main(argv):
    if argv[:1] == ["--latest"]:
        p = latest()
        print(p or "")
        return 0 if p else 1
    if argv[:1] == ["--check"]:
        doc = load(argv[1])
        brand = argv[2] if len(argv) > 2 else BRAND
        js = open(brand, encoding="utf-8").read()
        errs = check_docs(doc, open(DOCS, encoding="utf-8").read()) + check_brand(doc, js)
        for e in errs:
            print("  -", e, file=sys.stderr)
        return 1 if errs else 0
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    doc = load(argv[0])
    with open(DOCS, "w", encoding="utf-8") as f:
        f.write(md(doc))
    with open(BRAND, "r", encoding="utf-8") as f:
        js = f.read()
    with open(BRAND, "w", encoding="utf-8") as f:
        f.write(render_brand(js, doc))
    print(f"wrote {DOCS} and {BRAND}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
