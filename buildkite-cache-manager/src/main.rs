use anyhow::{Context, Result};
use clap::Parser;

use buildkite_cache_manager::cache::FsBackend;
use buildkite_cache_manager::cli::{Cli, Commands};
use buildkite_cache_manager::commands;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let backend = FsBackend;
    let cache_base =
        std::env::var("CACHE_BASE_URL").unwrap_or_else(|_| "/var/storagebox".to_string());
    let output = &cli.output;

    match &cli.command {
        Commands::Read {
            r#override,
            root,
            skip_dirs_create,
            input,
            output: out_path,
        } => {
            let build_id = get_build_id()?;
            let root = root.as_deref().unwrap_or(&build_id);
            commands::read::execute(
                &backend,
                &cache_base,
                root,
                input,
                out_path,
                *r#override,
                *skip_dirs_create,
                output,
            )
        }
        Commands::Write {
            r#override,
            root,
            input,
            output: out_path,
        } => {
            let build_id = get_build_id()?;
            let root = root.as_deref().unwrap_or(&build_id);
            commands::write::execute(
                &backend,
                &cache_base,
                root,
                input,
                out_path,
                *r#override,
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

fn get_build_id() -> Result<String> {
    std::env::var("BUILDKITE_BUILD_ID")
        .context("BUILDKITE_BUILD_ID must be set. This tool requires a Buildkite context.")
}
