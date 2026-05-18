use anyhow::Result;
use clap::Parser;

use deb_toolkit::builder::{build_debian_package, evaluate_and_validate};
use deb_toolkit::cli::{Cli, Command, LookupCommand, SessionCommand, VerifyCommand};
use deb_toolkit::session;
use deb_toolkit::{content_verifier, signature_verifier, signer, viewer};
use std::path::PathBuf;

fn init_logging(debug: bool) {
    let level = if debug { "debug" } else { "info" };
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .format_timestamp(None)
        .try_init();
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let result = dispatch(cli);
    if let Err(err) = result {
        log::error!("{:#}", err);
        std::process::exit(1);
    }
    Ok(())
}

fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Build(args) => {
            init_logging(args.debug);
            log::info!("Building debian package for {}...", args.package_name);
            let input = evaluate_and_validate(&args).map_err(|e| {
                log::error!("Validation phase failed: {}", e);
                e
            })?;
            build_debian_package(&input).map_err(|e| {
                log::error!("Building debian package failed: {}", e);
                e
            })?;
            log::info!(
                "Debian package for {} built successfully",
                args.package_name
            );
            Ok(())
        }
        Command::Sign(args) => {
            init_logging(args.debug);
            signer::sign(&args.deb, &args.key, args.debug)
        }
        Command::Verify { subcommand } => match subcommand {
            VerifyCommand::Content(args) => {
                init_logging(args.debug);
                log::info!("Verifying debian package {}...", args.deb);
                content_verifier::verify(&args)
            }
            VerifyCommand::Signature(args) => {
                init_logging(args.debug);
                signature_verifier::verify(&args.deb, args.key.as_deref(), args.debug)?;
                println!("Signature verified successfully");
                Ok(())
            }
        },
        Command::Lookup { subcommand } => match subcommand {
            LookupCommand::SignKey(args) => {
                init_logging(args.debug);
                let id = viewer::signature(&args.deb, args.debug)?;
                println!("{}", id);
                Ok(())
            }
        },
        Command::Session { subcommand } => {
            init_logging(false);
            dispatch_session(subcommand)
        }
    }
}

fn dispatch_session(cmd: SessionCommand) -> Result<()> {
    match cmd {
        SessionCommand::Open(args) => {
            session::open(
                std::path::Path::new(&args.input_deb),
                std::path::Path::new(&args.session_dir),
            )?;
            Ok(())
        }
        SessionCommand::Save(args) => {
            let session = session::Session::load(&args.session_dir)?;
            session::save(
                &session,
                std::path::Path::new(&args.output_deb),
                args.verify,
            )
        }
        SessionCommand::ReadField(args) => {
            let session = session::Session::load(&args.session_dir)?;
            let value = session.read_field(&args.field)?;
            println!("{}", value);
            Ok(())
        }
        SessionCommand::Insert(args) => {
            let session = session::Session::load(&args.session_dir)?;
            let sources: Vec<PathBuf> = args.sources.iter().map(PathBuf::from).collect();
            session.insert(&args.dest, &sources, args.directory)
        }
        SessionCommand::Remove(args) => {
            let session = session::Session::load(&args.session_dir)?;
            let n = session.remove(&args.pattern)?;
            log::info!("Removed {} file(s)", n);
            Ok(())
        }
        SessionCommand::Move(args) => {
            let session = session::Session::load(&args.session_dir)?;
            session.move_path(&args.source, &args.destination)
        }
        SessionCommand::Replace(args) => {
            let session = session::Session::load(&args.session_dir)?;
            let n = session.replace(&args.pattern, std::path::Path::new(&args.replacement))?;
            log::info!("Replaced {} file(s)", n);
            Ok(())
        }
        SessionCommand::RenamePackage(args) => {
            let session = session::Session::load(&args.session_dir)?;
            session.rename_package(&args.new_name)
        }
        SessionCommand::ReplaceSuite(args) => {
            let session = session::Session::load(&args.session_dir)?;
            session.replace_suite(&args.new_suite)
        }
        SessionCommand::Reversion(args) => {
            let session = session::Session::load(&args.session_dir)?;
            session.reversion(&args.new_version, args.update_deps)
        }
    }
}
