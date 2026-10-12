use anyhow::Result;
use clap::Parser;

use buildkite_cache_manager::cache::FsBackend;
use buildkite_cache_manager::cli::{Cli, Commands};
use buildkite_cache_manager::commands;

/// Exit code for a failed copy. Matches the bash manager.sh, so the CI retry
/// rule on exit status 2 keeps firing.
const EXIT_COPY_FAILED: i32 = 2;

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("Error: {e:?}");
        let code = if e.is::<MissingBuildId>() {
            1
        } else {
            EXIT_COPY_FAILED
        };
        std::process::exit(code);
    }
}

fn run(cli: Cli) -> Result<()> {
    let backend = FsBackend;
    let cache_base =
        std::env::var("CACHE_BASE_URL").unwrap_or_else(|_| "/var/storagebox".to_string());
    let output = &cli.output;

    match &cli.command {
        Commands::Read {
            no_clobber,
            root,
            skip_dirs_create,
            input,
            destination: out_path,
            ..
        } => {
            let build_id = get_build_id()?;
            // MINA_READ_CACHE_ROOT points reads at another build's root (set by
            // run-selection --from). Writes never honour it.
            let root = root
                .clone()
                .or_else(|| std::env::var("MINA_READ_CACHE_ROOT").ok())
                .unwrap_or(build_id);
            commands::read::execute(
                &backend,
                &cache_base,
                &root,
                input,
                out_path,
                !*no_clobber,
                *skip_dirs_create,
                output,
            )
        }
        Commands::Write {
            no_clobber,
            root,
            paths,
            ..
        } => {
            let build_id = get_build_id()?;
            let root = root.as_deref().unwrap_or(&build_id);
            // `num_args = 2..` guarantees at least one input and a destination.
            let (dest, inputs) = paths.split_last().expect("clap enforces at least 2 paths");
            commands::write::execute(
                &backend,
                &cache_base,
                root,
                inputs,
                dest,
                !*no_clobber,
                output,
            )
        }
        Commands::List { folder, debians } => {
            commands::list::execute(&backend, &cache_base, folder.as_deref(), *debians, output)
        }
        Commands::Prune {
            older_than,
            keep_latest_versions,
            keep_latest_timestamp,
            folder_type,
            dry_run,
            root,
        } => {
            let base = if let Some(r) = root {
                format!("{}/{}", cache_base, r)
            } else {
                cache_base.clone()
            };
            commands::prune::execute(
                &backend,
                &base,
                older_than.as_deref(),
                *keep_latest_versions,
                *keep_latest_timestamp,
                folder_type,
                *dry_run,
                output,
            )
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("BUILDKITE_BUILD_ID must be set. This tool requires a Buildkite context.")]
struct MissingBuildId;

fn get_build_id() -> Result<String> {
    std::env::var("BUILDKITE_BUILD_ID").map_err(|_| MissingBuildId.into())
}
