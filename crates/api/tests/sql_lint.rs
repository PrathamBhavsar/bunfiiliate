//! Runs the C03 SQL scope lint, its self-test, and the C04 findings guard under `cargo test`.
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn lint(args: &[&str]) -> (bool, String) {
    let out = Command::new("python3")
        .arg("-I")
        .arg("scripts/sql-scope-lint.py")
        .args(args)
        .current_dir(root())
        .output()
        .expect("python3");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn sql_lint_self_test_passes() {
    let (ok, out) = lint(&["--self-test"]);
    assert!(ok, "{out}");
    for f in [
        "format_built_sql_fails",
        "query_macro_fails",
        "multiline_missing_scope_fails",
    ] {
        assert!(out.contains(&format!("ok   {f}.rs")), "{f} missing: {out}");
    }
}

#[test]
fn sql_lint_is_clean() {
    let (ok, out) = lint(&[]);
    assert!(ok, "{out}");
}

#[test]
fn findings_md_has_no_open_items() {
    let md = std::fs::read_to_string(root().join("docs/audit/findings.md")).unwrap();
    assert!(!md.contains("status: open"), "open finding in findings.md");
}

#[test]
fn known_leaks_stays_empty() {
    let txt = std::fs::read_to_string(root().join("crates/api/tests/known_leaks.txt")).unwrap();
    let live: Vec<_> = txt
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .collect();
    assert!(live.is_empty(), "known_leaks.txt not empty: {live:?}");
}
