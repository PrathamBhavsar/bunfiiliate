#!/usr/bin/env bash
# Fails when the benchmark numbers quoted in the site copy drift from docs/benchmarks.md.
# docs/benchmarks.md is the published source (its table is generated from bench/results by bench/gen_docs.py).
# The site quotes `/r/:code` rps and p50 for both BF_FSYNC modes in site/src/brand.js ("facts" rows and the FAQ).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BRAND="$ROOT_DIR/site/src/brand.js"
DOCS="$ROOT_DIR/docs/benchmarks.md"

for f in "$BRAND" "$DOCS"; do
  [ -f "$f" ] || { echo "Error: $f not found." >&2; exit 1; }
done

echo "=== Verifying benchmark numbers in site/src/brand.js against docs/benchmarks.md ==="

python3 -I - "$BRAND" "$DOCS" <<'PY'
import re
import sys

brand = open(sys.argv[1], encoding="utf-8").read()
docs = open(sys.argv[2], encoding="utf-8").read()


def row(fsync):
    """rps and p50 (ms) of the `/r/:code` row for one BF_FSYNC mode."""
    for line in docs.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) >= 4 and cells[0] == "`/r/:code`" and cells[1] == fsync:
            # rps is written as `27700 (27000 to 28000)` once the docs come from a schema v1 result.
            return float(cells[2].split()[0]), float(cells[3].split()[0])
    sys.exit(f"no `/r/:code` row for BF_FSYNC={fsync} in docs/benchmarks.md")


def fmt_ms(v):
    return f"{v:.1f}".rstrip("0").rstrip(".") if v < 1 else f"{round(v)}"


def fmt_rps(v):
    return f"{v / 1000:.1f}k"


errors = []
for label, fsync in (("Tracker p50, fsync off", "off"), ("Tracker p50, fsync always", "always")):
    rps, p50 = row(fsync)
    expected = f"{fmt_ms(p50)} ms · {fmt_rps(rps)} req/s"
    m = re.search(r'\["' + re.escape(label) + r'",\s*"([^"]*)"\]', brand)
    if not m:
        errors.append(f"fact row '{label}' not found in brand.js")
    elif m.group(1) != expected:
        errors.append(f"'{label}': brand.js has '{m.group(1)}', docs/benchmarks.md gives '{expected}'")
    else:
        print(f"ok   {label}: {expected}")

# FAQ prose: "measured 0.4 ms p50" (off) and "p50 was about 13 ms" (always).
_, off_p50 = row("off")
_, on_p50 = row("always")
for pat, want, what in (
    (r"measured ([0-9.]+) ms p50", fmt_ms(off_p50), "FAQ fsync-off p50"),
    (r"p50 was about ([0-9.]+) ms", fmt_ms(on_p50), "FAQ fsync-always p50"),
):
    m = re.search(pat, brand)
    if not m:
        errors.append(f"{what} sentence not found in brand.js")
    elif m.group(1) != want:
        errors.append(f"{what}: brand.js has {m.group(1)} ms, docs/benchmarks.md gives {want} ms")
    else:
        print(f"ok   {what}: {want} ms")

if errors:
    print("\nBenchmark verification failed:", file=sys.stderr)
    for e in errors:
        print(f"  - {e}", file=sys.stderr)
    sys.exit(1)
print("\nAll benchmark numbers in site/src/brand.js match docs/benchmarks.md.")
PY
