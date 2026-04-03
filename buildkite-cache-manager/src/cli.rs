use clap::{Parser, Subcommand, ValueEnum};

/// Buildkite CI Cache Manager - read, write, list and prune cached artifacts
/// on Hetzner shared storage.
///
/// Requires BUILDKITE_BUILD_ID environment variable to be set (for read/write).
/// Cache root defaults to /var/storagebox, overridable via CACHE_BASE_URL.
#[derive(Parser, Debug)]
#[command(name = "buildkite-cache-manager", version, about)]
pub struct Cli {
    /// Output format for all commands (text or json)
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text, global = true)]
    pub output: OutputFormat,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Read (copy) file(s) from the CI cache to a local destination
    Read {
        /// Override existing cached files
        #[arg(short, long)]
        r#override: bool,

        /// Override cache root folder (default: BUILDKITE_BUILD_ID).
        /// Do not add leading/trailing slashes.
        #[arg(short, long)]
        root: Option<String>,

        /// Skip creating local directories
        #[arg(short, long)]
        skip_dirs_create: bool,

        /// Cache-relative path to read from (supports wildcards)
        input: String,

        /// Local destination path
        output: String,
    },

    /// Write (copy) local file(s) to the CI cache
    Write {
        /// Override existing files in cache
        #[arg(short, long)]
        r#override: bool,

        /// Override cache root folder (default: BUILDKITE_BUILD_ID).
        /// Do not add leading/trailing slashes.
        #[arg(short, long)]
        root: Option<String>,

        /// Local path to file(s) to write (supports wildcards)
        input: String,

        /// Cache-relative destination path
        output: String,
    },

    /// List files and folders in the CI cache
    List {
        /// Root folder to list. Can be a buildkite build ID or "legacy".
        /// If omitted, lists all top-level folders.
        #[arg()]
        folder: Option<String>,

        /// Show debian packages with codename/architecture structure
        #[arg(long)]
        debians: bool,
    },

    /// Prune (remove) cache folders based on age, version, or timestamp
    Prune {
        /// Remove folders older than this duration (e.g., "30d", "12h", "7d")
        #[arg(long)]
        older_than: Option<String>,

        /// Keep only the latest N folders by version (semver-like sorting)
        #[arg(long)]
        keep_latest_versions: Option<usize>,

        /// Keep only the latest N folders by modification timestamp
        #[arg(long)]
        keep_latest_timestamp: Option<usize>,

        /// Target folder type to prune
        #[arg(long, value_enum, default_value_t = FolderType::BuildId)]
        folder_type: FolderType,

        /// Perform a dry run without actually deleting anything
        #[arg(long)]
        dry_run: bool,

        /// Override cache root folder
        #[arg(short, long)]
        root: Option<String>,
    },
}

#[derive(Debug, Clone, ValueEnum, PartialEq)]
pub enum OutputFormat {
    /// Human-readable text output (tables for list, messages for read/write/prune)
    Text,
    /// JSON structured output
    Json,
}

#[derive(Debug, Clone, ValueEnum, PartialEq)]
pub enum FolderType {
    /// Buildkite build ID folders (UUID-like)
    BuildId,
    /// Legacy folder
    Legacy,
    /// All folder types
    All,
}
