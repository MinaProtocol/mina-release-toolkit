use clap::Args;

pub const DEFAULT_ARTIFACTS: &str = "mina-logproc,mina-archive,mina-rosetta,mina-daemon";
pub const DEFAULT_NETWORKS: &str = "devnet,mainnet";
pub const DEFAULT_CODENAMES: &str = "bullseye,focal";
pub const DEFAULT_DEBIAN_REPO: &str = "packages.o1test.net";
pub const DEFAULT_ARCHITECTURES: &str = "amd64";

#[derive(Args)]
pub struct PublishArgs {
    /// Comma separated list of artifacts to publish
    #[arg(long, default_value = DEFAULT_ARTIFACTS)]
    pub artifacts: String,

    /// Comma separated list of networks to publish
    #[arg(long, default_value = DEFAULT_NETWORKS)]
    pub networks: String,

    /// Buildkite build id of release build to publish
    #[arg(long)]
    pub buildkite_build_id: String,

    /// Source version of build to publish
    #[arg(long)]
    pub source_version: String,

    /// Target version of build to publish
    #[arg(long)]
    pub target_version: String,

    /// Comma separated list of debian codenames to publish
    #[arg(long, default_value = DEFAULT_CODENAMES)]
    pub codenames: String,

    /// Target debian channel
    #[arg(long)]
    pub channel: String,

    /// Publish to docker.io instead of gcr.io
    #[arg(long)]
    pub publish_to_docker_io: bool,

    /// Publish only docker images
    #[arg(long)]
    pub only_dockers: bool,

    /// Publish only debian packages
    #[arg(long)]
    pub only_debians: bool,

    /// Verify packages are published correctly
    #[arg(long)]
    pub verify: bool,

    /// Don't publish anything, just print what would be published
    #[arg(long)]
    pub dry_run: bool,

    /// Backend to use for storage
    #[arg(long, default_value = "gs")]
    pub backend: String,

    /// Debian repository to publish to
    #[arg(long, default_value = DEFAULT_DEBIAN_REPO)]
    pub debian_repo: String,

    /// Debian signing key to use
    #[arg(long)]
    pub debian_sign_key: Option<String>,

    /// Strip network from archive package name
    #[arg(long)]
    pub strip_network_from_archive: bool,

    /// Enable debug mode to show external command execution
    #[arg(long)]
    pub debug: bool,
}

#[derive(Args)]
pub struct PromoteArgs {
    /// Comma separated list of artifacts to promote
    #[arg(long, default_value = DEFAULT_ARTIFACTS)]
    pub artifacts: String,

    /// Comma separated list of networks
    #[arg(long, default_value = DEFAULT_NETWORKS)]
    pub networks: String,

    /// Source version of build
    #[arg(long)]
    pub source_version: String,

    /// Target version of build
    #[arg(long)]
    pub target_version: String,

    /// Comma separated list of debian codenames
    #[arg(long, default_value = DEFAULT_CODENAMES)]
    pub codenames: String,

    /// Source debian channel
    #[arg(long)]
    pub source_channel: Option<String>,

    /// Target debian channel
    #[arg(long)]
    pub target_channel: Option<String>,

    /// Publish to docker.io instead of gcr.io
    #[arg(long)]
    pub publish_to_docker_io: bool,

    /// Promote only docker images
    #[arg(long)]
    pub only_dockers: bool,

    /// Promote only debian packages
    #[arg(long)]
    pub only_debians: bool,

    /// Verify packages are promoted correctly
    #[arg(long)]
    pub verify: bool,

    /// Don't promote anything, just print what would be promoted
    #[arg(long)]
    pub dry_run: bool,

    /// Debian repository to promote to
    #[arg(long, default_value = DEFAULT_DEBIAN_REPO)]
    pub debian_repo: String,

    /// Debian signing key to use
    #[arg(long)]
    pub debian_sign_key: Option<String>,

    /// Strip network from archive package name
    #[arg(long)]
    pub strip_network_from_archive: bool,

    /// Enable debug mode to show external command execution
    #[arg(long)]
    pub debug: bool,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// Comma separated list of artifacts to verify
    #[arg(long, default_value = DEFAULT_ARTIFACTS)]
    pub artifacts: String,

    /// Comma separated list of networks
    #[arg(long, default_value = DEFAULT_NETWORKS)]
    pub networks: String,

    /// Target version to verify
    #[arg(long)]
    pub version: String,

    /// Comma separated list of debian codenames
    #[arg(long, default_value = DEFAULT_CODENAMES)]
    pub codenames: String,

    /// Target debian channel
    #[arg(long, default_value = "unstable")]
    pub channel: String,

    /// Debian repository to verify
    #[arg(long, default_value = DEFAULT_DEBIAN_REPO)]
    pub debian_repo: String,

    /// Verify in docker.io instead of gcr.io
    #[arg(long)]
    pub docker_io: bool,

    /// Verify only docker images
    #[arg(long)]
    pub only_dockers: bool,

    /// Verify only debian packages
    #[arg(long)]
    pub only_debians: bool,

    /// Debian repository is signed
    #[arg(long)]
    pub signed_debian_repo: bool,

    /// Docker suffix for verification
    #[arg(long)]
    pub docker_suffix: Option<String>,

    /// Enable debug mode to show external command execution
    #[arg(long)]
    pub debug: bool,
}

#[derive(Args)]
pub struct FixArgs {
    /// Comma separated list of debian codenames
    #[arg(long, default_value = DEFAULT_CODENAMES)]
    pub codenames: String,

    /// Target debian channel
    #[arg(long)]
    pub channel: String,

    /// Enable debug mode to show external command execution
    #[arg(long)]
    pub debug: bool,
}

#[derive(Args)]
pub struct PersistArgs {
    /// Backend to persist artifacts
    #[arg(long, default_value = "hetzner")]
    pub backend: String,

    /// Comma separated list of artifacts to persist
    #[arg(long, default_value = DEFAULT_ARTIFACTS)]
    pub artifacts: String,

    /// Buildkite build id
    #[arg(long)]
    pub buildkite_build_id: String,

    /// Target location to persist artifacts
    #[arg(long)]
    pub target: String,

    /// Codename for artifacts
    #[arg(long)]
    pub codename: String,

    /// New version for artifacts
    #[arg(long)]
    pub new_version: Option<String>,

    /// Suite for artifacts
    #[arg(long, default_value = "unstable")]
    pub suite: String,

    /// Enable debug mode to show external command execution
    #[arg(long)]
    pub debug: bool,
}

#[derive(Args)]
pub struct ProgressArgs {
    /// Target version to check (e.g. 3.0.0-rc1)
    #[arg(long)]
    pub version: String,

    /// Target release: alpha, beta, or stable
    #[arg(long)]
    pub release: String,

    /// Comma separated list of artifacts to check
    #[arg(long, default_value = DEFAULT_ARTIFACTS)]
    pub artifacts: String,

    /// Comma separated list of codenames to check
    #[arg(long, default_value = "bullseye,focal,jammy,noble,bookworm")]
    pub codenames: String,

    /// Build profile (e.g. lightnet, instrumented)
    #[arg(long)]
    pub profile: Option<String>,

    /// Check only debian packages
    #[arg(long)]
    pub only_debians: bool,

    /// Check only docker images
    #[arg(long)]
    pub only_dockers: bool,

    /// Skip checking packages.minaprotocol.com repositories
    #[arg(long)]
    pub skip_mina_public: bool,
}

#[derive(Args)]
pub struct ReversionArgs {
    /// Folder with `{codename}/*.deb` structure (typically the output of `pull`)
    #[arg(long)]
    pub source_folder: String,

    /// Folder to write reversioned debs into
    #[arg(long)]
    pub output_folder: String,

    /// New version string (e.g. 3.0.0-rc1)
    #[arg(long)]
    pub new_version: String,

    /// Replace the suite in the control file (e.g. stable, unstable)
    #[arg(long)]
    pub suite: Option<String>,

    /// Rename the package (e.g. mina-devnet-hardfork)
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Args)]
pub struct ValidateArgs {
    /// Comma separated list of debian codenames
    #[arg(long, default_value = DEFAULT_CODENAMES)]
    pub codenames: String,

    /// Target debian channel (required)
    #[arg(long)]
    pub channel: String,

    /// Comma separated list of architectures
    #[arg(long, default_value = DEFAULT_ARCHITECTURES)]
    pub archs: String,

    /// Debian repository bucket
    #[arg(long, default_value = DEFAULT_DEBIAN_REPO)]
    pub debian_repo: String,

    /// GPG key ID used to re-sign InRelease when fixing
    #[arg(long)]
    pub debian_sign_key: Option<String>,

    /// Repair broken manifests + re-sign InRelease + invalidate CDN cache
    #[arg(long)]
    pub fix: bool,

    /// Only list packages; skip SHA256 verification
    #[arg(long)]
    pub list_only: bool,
}

#[derive(Args)]
pub struct PullArgs {
    /// Backend to pull artifacts from
    #[arg(long, default_value = "hetzner")]
    pub backend: String,

    /// Comma separated list of artifacts to pull
    #[arg(long, default_value = DEFAULT_ARTIFACTS)]
    pub artifacts: String,

    /// Buildkite build id
    #[arg(long)]
    pub buildkite_build_id: String,

    /// Target local location
    #[arg(long, default_value = ".")]
    pub target: String,

    /// Comma separated list of codenames
    #[arg(long, default_value = DEFAULT_CODENAMES)]
    pub codenames: String,

    /// Comma separated list of networks
    #[arg(long, default_value = DEFAULT_NETWORKS)]
    pub networks: String,

    /// Enable debug mode to show external command execution
    #[arg(long)]
    pub debug: bool,
}
