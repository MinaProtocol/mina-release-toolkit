use clap::{Parser, Subcommand};
use colored::*;
use std::env;

use release_manager::cli::*;
use release_manager::commands;
use release_manager::errors::ManagerResult;
use release_manager::utils;

#[derive(Parser)]
#[command(name = "release-manager")]
#[command(about = "Mina Protocol Release Manager - Comprehensive release management functionality")]
#[command(version = "1.0.0")]
#[command(long_about = r#"
This tool provides comprehensive release management functionality for the Mina Protocol project.
It handles the complete lifecycle of build artifacts including publishing, promotion, verification,
and maintenance of packages across different channels and platforms.

Main capabilities:
- PUBLISH: Put already-built .deb files into a Debian repository, unchanged, and
  verify afterwards that each one is really there
- PUBLISH-FROM-CACHE: Legacy. Pull from the CI cache by buildkite build id and
  re-version on the way
- PROMOTE: Promote artifacts from one channel/registry to another (e.g., unstable -> stable)
- VERIFY: Verify that artifacts are correctly published in target channels/registries
- FIX: Repair Debian repository manifests when needed
- PERSIST: Archive artifacts to long-term storage backends

Supported artifacts: mina-daemon, mina-archive, mina-rosetta, mina-logproc
Supported networks: devnet, mainnet
Supported platforms: Debian (bullseye, focal), Docker (GCR, Docker.io)
Supported channels: unstable, alpha, beta, stable
Supported backends: Google Cloud Storage (gs), Hetzner, local filesystem
"#)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, env = "RUST_LOG", default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Publish already-built .deb files to a debian repository, unchanged
    Publish(PublishArgs),
    /// Legacy: publish from the CI cache by buildkite build id, re-versioning
    /// on the way. Kept as the fallback for flows that still need a rewrite
    PublishFromCache(PublishFromCacheArgs),
    /// Promote artifacts from one channel/registry to another
    Promote(PromoteArgs),
    /// Verify artifacts in target channel/registry
    Verify(VerifyArgs),
    /// Fix debian package repository manifests
    Fix(FixArgs),
    /// Validate a debian channel: list, SHA256-check, optionally repair + re-sign + invalidate CDN
    Validate(ValidateArgs),
    /// Persist artifacts to long-term storage
    Persist(PersistArgs),
    /// Pull artifacts from cache to local directory
    Pull(PullArgs),
    /// Reversion every .deb in a folder (as produced by `pull`)
    Reversion(ReversionArgs),
    /// Show release-progress report (what's published per channel/codename/arch)
    Progress(ProgressArgs),
}

#[tokio::main]
async fn main() -> ManagerResult<()> {
    let cli = Cli::parse();

    // Initialize logger
    env::set_var("RUST_LOG", &cli.log_level);
    env_logger::init();

    // Check required applications based on command
    check_prerequisites(&cli.command).await?;

    let result = match cli.command {
        Commands::Publish(args) => commands::publish::execute(args).await,
        Commands::PublishFromCache(args) => commands::publish_from_cache::execute(args).await,
        Commands::Promote(args) => commands::promote::execute(args).await,
        Commands::Verify(args) => commands::verify::execute(args).await,
        Commands::Fix(args) => commands::fix::execute(args).await,
        Commands::Validate(args) => commands::validate::execute(args).await,
        Commands::Persist(args) => commands::persist::execute(args).await,
        Commands::Pull(args) => commands::pull::execute(args).await,
        Commands::Reversion(args) => commands::reversion::execute(args).await,
        Commands::Progress(args) => commands::progress::execute(args).await,
    };

    match result {
        Ok(_) => {
            println!("{}", " ✅  Operation completed successfully.".green());
            Ok(())
        }
        Err(e) => {
            eprintln!("{} {}", "❌".red(), e.to_string().red());
            std::process::exit(1);
        }
    }
}

async fn check_prerequisites(command: &Commands) -> ManagerResult<()> {
    use utils::check_app;

    match command {
        Commands::Publish(_) => {
            check_app("deb-s3").await?;
        }
        Commands::PublishFromCache(args) => {
            if args.backend == "gs" {
                check_app("gcloud").await?;
            }
            if args.verify {
                check_app("docker").await?;
            }
        }
        Commands::Promote(args) => {
            if args.verify {
                check_app("docker").await?;
            }
        }
        Commands::Verify(_) => {
            check_app("docker").await?;
        }
        Commands::Fix(_) => {
            check_app("deb-s3").await?;
        }
        Commands::Validate(_) => {
            check_app("deb-s3").await?;
        }
        _ => {}
    }

    Ok(())
}
