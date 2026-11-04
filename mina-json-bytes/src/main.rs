use mina_json_bytes::{decode, encode, jq};
use std::ffi::OsString;
use std::io;
use std::process::ExitCode;

const USAGE: &str = "\
Carry bytes that are not valid UTF-8 through jq.

Usage:
  mina-json-bytes encode < input > output
  mina-json-bytes decode < input > output
  mina-json-bytes jq [jq options] <filter> < input > output

encode replaces each invalid byte with the token __MINA_BYTE_xx__, and decode
replaces each token with its byte. jq does encode | jq | decode. It takes JSON
from stdin or from --slurpfile only, and returns the exit code of jq.

Exit codes: 1 for a failure, 2 for a usage error, or the exit code of jq.
";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    let rest: Vec<OsString> = args.collect();
    let result = match (command.as_ref().and_then(|c| c.to_str()), rest.is_empty()) {
        (Some("--help" | "-h"), _) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        (Some("--version" | "-V"), _) => {
            println!("mina-json-bytes {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        (Some("encode"), true) => encode(io::stdin().lock(), io::stdout().lock()).map(|()| 0),
        (Some("decode"), true) => decode(io::stdin().lock(), io::stdout().lock()).map(|()| 0),
        (Some("jq"), _) => match jq::plan(&rest) {
            Ok(plan) => jq::run(plan),
            Err(message) => return usage_error(&message),
        },
        _ => return usage_error("expected encode, decode or jq"),
    };
    match result {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("mina-json-bytes: {error}");
            ExitCode::from(1)
        }
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("mina-json-bytes: {message}\n\n{USAGE}");
    ExitCode::from(2)
}
