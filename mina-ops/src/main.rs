use std::path::PathBuf;

use clap::{Parser, Subcommand};

use mina_ops::config::Registry;
use mina_ops::error::OpsResult;
use mina_ops::inventory::{self, InventoryQuery};
use mina_ops::report;

#[derive(Parser)]
#[command(name = "mina-ops")]
#[command(version)]
#[command(about = "Read-only inventory across Mina CI, artifact and release systems")]
#[command(long_about = r#"
Answers one question that no single system can: for a given commit, which
Buildkite builds ran, which Debian packages exist in which repository, and
which Docker images were pushed.

Nothing is published, promoted or deleted here — that is release-manager's
work. Credentials are the ones already on this machine: a Buildkite API token
from the environment, aws for the Debian repositories, docker for the
registries.
"#)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Project registry to use instead of the built-in one.
    #[arg(long, global = true, env = "MINA_OPS_CONFIG")]
    config: Option<PathBuf>,

    /// Project in the registry.
    #[arg(long, global = true, default_value = "mina")]
    project: String,

    /// Print JSON instead of a report.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// What exists where, for one commit or version
    Artifacts(ArtifactsArgs),
    /// Buildkite builds for one commit
    Builds(BuildsArgs),
}

#[derive(clap::Args)]
struct ArtifactsArgs {
    /// Commit to report on. A short hash needs --repo-path to be expanded.
    #[arg(long)]
    commit: Option<String>,

    /// Package version, when it is already known. Skips version resolution.
    #[arg(long)]
    version: Option<String>,

    /// Release channel, which selects the repositories and the network.
    #[arg(long)]
    channel: Option<String>,

    /// Comma-separated artifacts. Defaults to the project's list.
    #[arg(long)]
    artifacts: Option<String>,

    /// Comma-separated Debian codenames. Defaults to the project's list.
    #[arg(long)]
    codenames: Option<String>,

    /// Network, when it differs from the channel's default.
    #[arg(long)]
    network: Option<String>,

    /// Build profile: lightnet or instrumented.
    #[arg(long)]
    profile: Option<String>,

    /// Local checkout used to expand a short commit hash.
    #[arg(long, env = "MINA_REPO")]
    repo_path: Option<PathBuf>,

    /// Builds whose artifacts are listed. Buildkite allows 200 requests a minute.
    #[arg(long, default_value_t = 5)]
    max_builds: usize,

    #[arg(long)]
    skip_buildkite: bool,

    #[arg(long)]
    skip_debian: bool,

    #[arg(long)]
    skip_docker: bool,
}

#[derive(clap::Args)]
struct BuildsArgs {
    /// Commit to look up. A short hash needs --repo-path to be expanded.
    #[arg(long)]
    commit: String,

    /// Local checkout used to expand a short commit hash.
    #[arg(long, env = "MINA_REPO")]
    repo_path: Option<PathBuf>,

    /// Builds whose artifacts are listed.
    #[arg(long, default_value_t = 10)]
    max_builds: usize,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: &Cli) -> OpsResult<()> {
    let (registry, source) = Registry::load(cli.config.as_deref())?;
    let project = registry.project(&cli.project)?;

    match &cli.command {
        Command::Artifacts(args) => {
            let commit = match &args.commit {
                Some(c) => Some(inventory::resolve_commit(c, args.repo_path.as_deref())?),
                None => None,
            };
            if commit.is_none() && args.version.is_none() {
                return Err(mina_ops::error::OpsError::Config(
                    "pass --commit, --version, or both".to_string(),
                ));
            }

            let query = InventoryQuery {
                commit,
                version: args.version.clone(),
                channel: args
                    .channel
                    .clone()
                    .unwrap_or_else(|| project.defaults.channel.clone()),
                artifacts: split_list(args.artifacts.as_deref())
                    .unwrap_or_else(|| project.defaults.artifacts.clone()),
                codenames: split_list(args.codenames.as_deref())
                    .unwrap_or_else(|| project.defaults.codenames.clone()),
                network: args.network.clone(),
                profile: args
                    .profile
                    .clone()
                    .or_else(|| project.defaults.profile.clone()),
                max_builds: args.max_builds,
                skip_buildkite: args.skip_buildkite,
                skip_debian: args.skip_debian,
                skip_docker: args.skip_docker,
            };

            let inventory = inventory::collect(&cli.project, project, &query).await?;
            emit(cli, &inventory)?;
        }
        Command::Builds(args) => {
            let commit = inventory::resolve_commit(&args.commit, args.repo_path.as_deref())?;
            let query = InventoryQuery {
                commit: Some(commit),
                version: None,
                channel: project.defaults.channel.clone(),
                artifacts: Vec::new(),
                codenames: Vec::new(),
                network: None,
                profile: None,
                max_builds: args.max_builds,
                skip_buildkite: false,
                skip_debian: true,
                skip_docker: true,
            };
            let inventory = inventory::collect(&cli.project, project, &query).await?;
            emit(cli, &inventory)?;
        }
    }

    if !cli.json {
        eprintln!("registry: {source}");
    }
    Ok(())
}

fn emit(cli: &Cli, inventory: &mina_ops::model::Inventory) -> OpsResult<()> {
    if cli.json {
        let json = serde_json::to_string_pretty(inventory)
            .map_err(|e| mina_ops::error::OpsError::Other(e.to_string()))?;
        println!("{json}");
    } else {
        print!("{}", report::render(inventory));
    }
    Ok(())
}

fn split_list(value: Option<&str>) -> Option<Vec<String>> {
    value.map(|v| {
        v.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
}
