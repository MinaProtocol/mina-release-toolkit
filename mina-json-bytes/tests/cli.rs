//! Runs the binary with a real jq, as the hardfork scripts do.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const LEDGER: &[u8] = b"[{\"pk\":\"B62a\",\"token_symbol\":\"\xff\",\"balance\":\"1\"},\
{\"pk\":\"B62b\",\"token_symbol\":\"\xe2\x82\xac\",\"zkapp\":{\"zkapp_uri\":\"a\xc3\"}}]";

fn run(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mina-json-bytes"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        // jq can stop before it reads all of the input.
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    output
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

#[test]
fn encode_then_decode_returns_the_input() {
    let encoded = run(&["encode"], LEDGER);
    assert!(encoded.status.success());
    assert!(std::str::from_utf8(&encoded.stdout).is_ok());
    let decoded = run(&["decode"], &encoded.stdout);
    assert!(decoded.status.success());
    assert_eq!(decoded.stdout, LEDGER);
}

#[test]
fn encode_fails_on_the_prefix_in_the_input() {
    let output = run(&["encode"], b"\"__MINA_BYTE_ff__\"");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("__MINA_BYTE_"));
}

#[test]
fn jq_filter_keeps_the_bytes() {
    let output = run(&["jq", "-c", "."], LEDGER);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(output.stdout.strip_suffix(b"\n").unwrap(), LEDGER);
}

#[test]
fn jq_del_genesis_keeps_the_bytes() {
    let input = b"{\"genesis\":{\"a\":1},\"ledger\":{\"accounts\":[{\"s\":\"\xff\"}]}}";
    let output = run(&["jq", "-c", "del(.genesis)"], input);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        output.stdout,
        b"{\"ledger\":{\"accounts\":[{\"s\":\"\xff\"}]}}\n"
    );
}

#[test]
fn jq_argjson_and_arg_values_are_not_taken_for_files() {
    let output = run(
        &[
            "jq",
            "-c",
            "--arg",
            "b",
            "2",
            "--argjson",
            "new",
            "[1]",
            ".[0].balance=$b | . + $new",
        ],
        LEDGER,
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(count(&output.stdout, b"\xff"), 1);
    assert_eq!(count(&output.stdout, b"\"balance\":\"2\""), 1);
}

#[test]
fn jq_slurpfile_inputs_are_encoded() {
    let dir = std::env::temp_dir().join(format!("mina-json-bytes-test.{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("genesis.json");
    std::fs::write(&path, LEDGER).unwrap();
    let output = run(
        &[
            "jq",
            "-n",
            "-c",
            "--slurpfile",
            "genesis",
            path.to_str().unwrap(),
            "--slurpfile",
            "next",
            path.to_str().unwrap(),
            "{ledger:{accounts:$genesis[0]},next:{accounts:$next[0]}}",
        ],
        b"",
    );
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(count(&output.stdout, b"\xff"), 2);
    assert_eq!(count(&output.stdout, b"\xef\xbf\xbd"), 0);
}

#[test]
fn jq_exit_code_is_returned() {
    let output = run(&["jq", "-e", ".missing"], b"{}");
    assert_eq!(output.status.code(), Some(1));
    let output = run(&["jq", "this is not a filter"], b"{}");
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn jq_failure_on_invalid_json_is_not_hidden() {
    let output = run(&["jq", "."], b"{");
    assert_ne!(output.status.code(), Some(0));
}

#[test]
fn jq_rejects_inputs_it_would_not_encode() {
    for args in [
        &["jq", ".", "ledger.json"][..],
        &["jq", "--rawfile", "a", "ledger.json", "."],
        &["jq", "--args", ".", "a"],
        &["jq", "-f", "filter.jq"],
        &["jq", "--from-file", "filter.jq"],
        &["jq", "-c"],
        &["jq", "--arg", "a"],
    ] {
        let output = run(args, b"{}");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn unknown_command_is_a_usage_error() {
    assert_eq!(run(&[], b"").status.code(), Some(2));
    assert_eq!(run(&["recode"], b"").status.code(), Some(2));
    assert_eq!(run(&["encode", "file.json"], b"").status.code(), Some(2));
}

#[test]
fn missing_slurpfile_is_a_failure() {
    let path = Path::new("/nonexistent/ledger.json");
    let output = run(
        &["jq", "-n", "--slurpfile", "a", path.to_str().unwrap(), "$a"],
        b"",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("ledger.json"));
}

#[test]
fn jq_fails_on_the_prefix_in_stdin() {
    let output = run(&["jq", "."], b"\"__MINA_BYTE_ff__\"");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("__MINA_BYTE_"));
}
