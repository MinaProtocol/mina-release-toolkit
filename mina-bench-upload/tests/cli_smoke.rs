//! Black-box CLI smoke tests.
//!
//! Exercise the compiled binary against each fixture format with
//! `--dry-run` (no network, but proves the full parse → ready-to-upload
//! pipeline works for every supported format).
//!
//! Wiremock-backed upload/regression tests live separately; they
//! require an async test harness and a stub InfluxDB instance. The
//! intent here is just to confirm "every parser plugs into the CLI
//! without panic", which is the regression we'd most likely break.

use std::process::Command;

fn bin() -> String {
    env!("CARGO_BIN_EXE_mina-bench-upload").to_string()
}

fn run_dry_run(format: &str, fixture: &str) -> (bool, String, String) {
    let input = format!("tests/fixtures/{}", fixture);
    let out = Command::new(bin())
        .args([
            "--format",
            format,
            "--input",
            &input,
            "--branch",
            "develop",
            "--upload",
            "--check-regression",
            "--dry-run",
        ])
        .output()
        .expect("spawn mina-bench-upload");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn dry_run_mina_base() {
    let (ok, _, stderr) = run_dry_run("mina-base", "janestreet.txt");
    assert!(ok, "stderr:\n{}", stderr);
    assert!(stderr.contains("Parsed 3 record(s)"), "stderr:\n{}", stderr);
    assert!(
        stderr.contains("[dry-run] would upload 3"),
        "stderr:\n{}",
        stderr
    );
}

#[test]
fn dry_run_ledger_export() {
    let (ok, _, stderr) = run_dry_run("ledger-export", "janestreet.txt");
    assert!(ok, "stderr:\n{}", stderr);
    assert!(stderr.contains("Parsed 3 record(s)"));
}

#[test]
fn dry_run_snark() {
    let (ok, _, stderr) = run_dry_run("snark", "snark.txt");
    assert!(ok, "stderr:\n{}", stderr);
    assert!(stderr.contains("Parsed 4 record(s)"));
}

#[test]
fn dry_run_zkapp() {
    let (ok, _, stderr) = run_dry_run("zkapp", "zkapp.txt");
    assert!(ok, "stderr:\n{}", stderr);
    assert!(stderr.contains("Parsed 5 record(s)"));
}

#[test]
fn dry_run_archive() {
    let (ok, _, stderr) = run_dry_run("archive", "archive.json");
    assert!(ok, "stderr:\n{}", stderr);
    assert!(stderr.contains("Parsed 3 record(s)"));
}

#[test]
fn dry_run_heap() {
    let (ok, _, stderr) = run_dry_run("heap", "heap.txt");
    assert!(ok, "stderr:\n{}", stderr);
    assert!(stderr.contains("Parsed 4 record(s)"));
}

#[test]
fn dry_run_ledger_apply() {
    let (ok, _, stderr) = run_dry_run("ledger-apply", "ledger_apply.json");
    assert!(ok, "stderr:\n{}", stderr);
    assert!(stderr.contains("Parsed 1 record(s)"));
}

#[test]
fn parse_error_exits_2() {
    let out = Command::new(bin())
        .args([
            "--format",
            "archive",
            "--input",
            "tests/fixtures/heap.txt", // wrong format for this input
            "--branch",
            "develop",
            "--dry-run",
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2), "wanted EXIT_PARSE_ERROR (2)");
}

#[test]
fn unknown_format_rejected_by_clap() {
    let out = Command::new(bin())
        .args([
            "--format",
            "not-a-format",
            "--input",
            "tests/fixtures/heap.txt",
            "--branch",
            "develop",
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    // clap's own error code is typically 2 for usage errors; we
    // don't assert the exact code, just that it didn't run.
    assert!(String::from_utf8_lossy(&out.stderr).contains("not-a-format"));
}
