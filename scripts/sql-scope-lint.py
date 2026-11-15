#!/usr/bin/env python3
"""Tenant-scope lint for sqlx SQL (C03). Stdlib only: run with `python -I`.

Usage:
  python -I scripts/sql-scope-lint.py             # lint crates/db/src + crates/api/src
  python -I scripts/sql-scope-lint.py --self-test # run scripts/sql-lint-fixtures

Rule: every SELECT/UPDATE/DELETE that names a tenant table must mention
`workspace_id`, or carry `-- scope: <reason>` (in the SQL) or `// scope: <reason>`
(within 3 lines above the call), or sit in a fn listed in
docs/audit/sql-lint-allowlist.toml (reason required, stale entries fail).

LIMITS (see docs/audit/sql-lint.md): the lint reads only string literals passed
directly to `sqlx::query*`. It cannot see inside `format!`/`concat!`/const/variable
built SQL or `query!` macros. Those FAIL CLOSED ("opaque") and need an allowlist
entry or a `scope:` comment, justified by a human. Multi-line literals are parsed.
"""
import os, re, sys, tomllib

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
SRC_DIRS = ["crates/db/src", "crates/api/src"]
ALLOWLIST = "docs/audit/sql-lint-allowlist.toml"
FIXTURES = "scripts/sql-lint-fixtures"
EXTRA_TENANT = """clicks conversions ledger_entries payouts payout_batches affiliates customers
referral_codes webhook_endpoints webhook_deliveries notifications audit_log email_templates
contact_requests daily_rollups rollups programs affiliate_programs""".split()

CALL = re.compile(r"(?:sqlx::)?query(?:_as|_scalar)?(?P<macro>!)?\s*(?:::<[^()]*?>)?\s*\(")
FN = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
SQL_WRITE_READ = re.compile(r"^\s*(select|update|delete|with)\b", re.I)


def tenant_tables():
    names = set(EXTRA_TENANT)
    mig = os.path.join(ROOT, "crates/db/migrations")
    if os.path.isdir(mig):
        for f in sorted(os.listdir(mig)):
            txt = open(os.path.join(mig, f)).read()
            for m in re.finditer(r"CREATE TABLE (?:IF NOT EXISTS )?(\w+)\s*\((.*?)\n\);", txt, re.S | re.I):
                if "workspace_id" in m.group(2):
                    names.add(m.group(1))
    return names


def read_string(src, i):
    """src[i] is the start of a string literal; return (value, end_index) or None."""
    m = re.compile(r'r(#*)"').match(src, i)
    if m:
        close = '"' + m.group(1)
        j = src.find(close, m.end())
        return (src[m.end():j], j + len(close)) if j >= 0 else None
    if src[i] != '"':
        return None
    out, j = [], i + 1
    while j < len(src):
        c = src[j]
        if c == "\\":
            n = src[j + 1]
            if n == "\n":  # line continuation eats following whitespace
                j += 2
                while j < len(src) and src[j] in " \t\r\n":
                    j += 1
                continue
            out.append({"n": "\n", "t": "\t", '"': '"', "\\": "\\"}.get(n, n))
            j += 2
            continue
        if c == '"':
            return "".join(out), j + 1
        out.append(c)
        j += 1
    return None


def call_args(src, i):
    """i is just after '('. Return (arg_text, end) up to matching ')', string aware."""
    depth, j = 1, i
    while j < len(src):
        c = src[j]
        if c == '"' or (c == "r" and re.compile(r'r#*"').match(src, j) and not src[j - 1].isalnum() and src[j - 1] != "_"):
            s = read_string(src, j)
            if not s:
                break
            j = s[1]
            continue
        if c == "/" and src.startswith("//", j):
            j = src.find("\n", j)
            if j < 0:
                break
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                return src[i:j], j
        j += 1
    return src[i:], len(src)


def scan_file(path, rel, tables):
    src = open(path).read()
    findings = []  # dict(file, fn, line, kind, detail)
    fns = [(m.start(), m.group(1)) for m in FN.finditer(src)]
    tbl_re = re.compile(r"\b(" + "|".join(sorted(tables, key=len, reverse=True)) + r")\b", re.I)
    for m in CALL.finditer(src):
        # skip matches inside comments
        ls = src.rfind("\n", 0, m.start()) + 1
        if src[ls:m.start()].lstrip().startswith("//"):
            continue
        # skip `fn query(` style definitions
        if re.search(r"\bfn\s+$", src[max(0, m.start() - 4):m.start()]):
            continue
        line = src.count("\n", 0, m.start()) + 1
        fn = next((n for pos, n in reversed(fns) if pos < m.start()), "<module>")
        args, _ = call_args(src, m.end())
        above = "\n".join(src.splitlines()[max(0, line - 4):line])
        slash_scope = re.search(r"//\s*scope:\s*\S", above + "\n" + args) is not None
        base = dict(file=rel, fn=fn, line=line)
        if m.group("macro"):
            findings.append(dict(base, kind="opaque", detail="query! macro SQL is invisible to the lint", ok=slash_scope))
            continue
        a = args.strip()
        a = a[1:].lstrip() if a.startswith("&") else a
        lit = read_string(a, 0) if a[:1] in 'r"' else None
        if not lit or a[lit[1]:].strip().strip(",") != "":
            findings.append(dict(base, kind="opaque", detail="non-literal SQL (format!/const/variable) is invisible to the lint", ok=slash_scope))
            continue
        sql = lit[0]
        if not SQL_WRITE_READ.match(sql) or not tbl_re.search(sql):
            continue
        sql_scope = re.search(r"--\s*scope:\s*\S", sql) is not None
        if "workspace_id" in sql or sql_scope or slash_scope:
            continue
        t = sorted({x.lower() for x in tbl_re.findall(sql)})
        findings.append(dict(base, kind="unscoped", detail="touches " + ",".join(t) + " without workspace_id", ok=False))
    return findings


def load_allow(path):
    if not os.path.exists(path):
        return [], []
    data = tomllib.load(open(path, "rb"))
    entries, errs = [], []
    for e in data.get("allow", []):
        if not str(e.get("reason", "")).strip():
            errs.append(f"allowlist entry {e.get('file')}::{e.get('fn')} has no reason")
        entries.append(e)
    return entries, errs


def lint(base, dirs, allow_path):
    tables = tenant_tables()
    findings = []
    for d in dirs:
        full = os.path.join(base, d)
        for dp, _, fs in os.walk(full):
            for f in sorted(fs):
                if f.endswith(".rs"):
                    p = os.path.join(dp, f)
                    findings += scan_file(p, os.path.relpath(p, base), tables)
    entries, errs = load_allow(allow_path)
    used = set()
    bad = []
    for f in findings:
        if f["ok"]:
            continue
        hit = [i for i, e in enumerate(entries) if e.get("file") == f["file"] and e.get("fn") == f["fn"]]
        if hit:
            used.update(hit)
        else:
            bad.append(f"{f['file']}:{f['line']}: fn {f['fn']}: {f['kind']}: {f['detail']}")
    for i, e in enumerate(entries):
        if i not in used:
            errs.append(f"stale allowlist entry {e.get('file')}::{e.get('fn')} matches no flagged query")
    scoped_comments = sum(1 for f in findings if f["ok"])
    return bad + errs, scoped_comments, len(entries)


def self_test():
    fx = os.path.join(ROOT, FIXTURES)
    fails = 0
    for name in sorted(os.listdir(fx)):
        if not name.endswith(".rs"):
            continue
        work = os.path.join(fx, name)
        head = open(work).readline()
        m = re.match(r"//\s*expect:\s*(pass|fail)", head)
        assert m, f"{name}: missing '// expect:' header"
        allow = work[:-3] + ".toml"
        src_dir = os.path.join(fx, "_" + name[:-3])
        os.makedirs(src_dir, exist_ok=True)
        open(os.path.join(src_dir, name), "w").write(open(work).read())
        try:
            probs, _, _ = lint(fx, ["_" + name[:-3]], allow)
        finally:
            os.remove(os.path.join(src_dir, name))
            os.rmdir(src_dir)
        got = "fail" if probs else "pass"
        ok = got == m.group(1)
        print(f"{'ok  ' if ok else 'FAIL'} {name}: expected {m.group(1)}, got {got}")
        fails += not ok
    return fails


def main():
    if "--self-test" in sys.argv:
        sys.exit(1 if self_test() else 0)
    probs, comments, n = lint(ROOT, SRC_DIRS, os.path.join(ROOT, ALLOWLIST))
    for p in probs:
        print(p)
    print(f"sql-lint: {len(probs)} problem(s); {comments} inline scope comment(s); {n} allowlist entr{'y' if n == 1 else 'ies'}")
    sys.exit(1 if probs else 0)


main()
