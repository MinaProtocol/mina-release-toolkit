use clap::{Args, Parser, Subcommand};
use mina_json_bytes::jq::JqInvocation;
use mina_json_bytes::{Decoder, Encoder, Result, Transcode};
use std::ffi::OsString;
use std::io;
use std::process::ExitCode;

/// Carry bytes that are not valid UTF-8 through jq.
#[derive(Parser)]
#[command(name = "mina-json-bytes", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Replace each invalid byte of stdin with the token __MINA_BYTE_xx__
    Encode,
    /// Replace each token of stdin with its byte
    Decode,
    /// Run encode | jq | decode, and return the exit code of jq
    Jq(JqArgs),
}

#[derive(Args)]
struct JqArgs {
    /// The jq options and filter. JSON comes from stdin or --slurpfile only
    #[arg(
        value_name = "JQ_ARGUMENT",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    arguments: Vec<OsString>,
}

impl Command {
    /// Runs the command and returns the exit code.
    fn run(self) -> Result<u8> {
        match self {
            Self::Encode => Encoder::new(io::stdout().lock()).run(io::stdin().lock())?,
            Self::Decode => Decoder::new(io::stdout().lock()).run(io::stdin().lock())?,
            Self::Jq(args) => return JqInvocation::parse(&args.arguments)?.run(),
        }
        Ok(0)
    }
}

fn main() -> ExitCode {
    match Cli::parse().command.run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("mina-json-bytes: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}
