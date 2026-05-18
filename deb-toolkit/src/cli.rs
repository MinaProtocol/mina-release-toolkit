use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "deb-builder",
    version,
    about = "Generate, sign, and verify Debian packages"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Build a debian package from a build directory
    Build(BuildArgs),
    /// Sign an existing debian package
    Sign(SignArgs),
    /// Verify details of a debian package
    Verify {
        #[command(subcommand)]
        subcommand: VerifyCommand,
    },
    /// Look up details from a debian package
    Lookup {
        #[command(subcommand)]
        subcommand: LookupCommand,
    },
    /// Transactional .deb modification (open / mutate / save)
    Session {
        #[command(subcommand)]
        subcommand: SessionCommand,
    },
}

/// Verbs accepted under `deb-toolkit session …`. Mirror the bash
/// `deb-session-*.sh` family on mina's develop branch.
#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// Extract a .deb into a session directory for modification
    Open(SessionOpenArgs),
    /// Repack a session directory into a fresh .deb
    Save(SessionSaveArgs),
    /// Print the value of a control-file field
    #[command(name = "read-field")]
    ReadField(SessionReadFieldArgs),
    /// Insert one or more files into the package
    Insert(SessionInsertArgs),
    /// Remove file(s) matching a pattern from the package
    Remove(SessionRemoveArgs),
    /// Move / rename a file within the package
    Move(SessionMoveArgs),
    /// Replace file(s) matching a pattern with a new file
    Replace(SessionReplaceArgs),
    /// Rename the package (Package: field)
    #[command(name = "rename-package")]
    RenamePackage(SessionRenamePackageArgs),
    /// Set the Suite: field
    #[command(name = "replace-suite")]
    ReplaceSuite(SessionReplaceSuiteArgs),
    /// Set the Version: field, optionally rewriting dep version constraints
    Reversion(SessionReversionArgs),
}

#[derive(Args, Debug)]
pub struct SessionOpenArgs {
    /// Path to the .deb file to open
    pub input_deb: String,
    /// Directory where the session will be created
    pub session_dir: String,
}

#[derive(Args, Debug)]
pub struct SessionSaveArgs {
    /// Session directory created by `session open`
    pub session_dir: String,
    /// Output path for the generated .deb
    pub output_deb: String,
    /// Verify the produced package with `dpkg-deb --info`
    #[arg(long = "verify", default_value_t = false)]
    pub verify: bool,
}

#[derive(Args, Debug)]
pub struct SessionReadFieldArgs {
    pub session_dir: String,
    pub field: String,
}

#[derive(Args, Debug)]
pub struct SessionInsertArgs {
    /// Treat `dest` as a directory (files keep their names)
    #[arg(short = 'd', long = "directory", default_value_t = false)]
    pub directory: bool,
    pub session_dir: String,
    pub dest: String,
    /// One or more source files to insert
    #[arg(num_args = 1..)]
    pub sources: Vec<String>,
}

#[derive(Args, Debug)]
pub struct SessionRemoveArgs {
    pub session_dir: String,
    pub pattern: String,
}

#[derive(Args, Debug)]
pub struct SessionMoveArgs {
    pub session_dir: String,
    pub source: String,
    pub destination: String,
}

#[derive(Args, Debug)]
pub struct SessionReplaceArgs {
    pub session_dir: String,
    pub pattern: String,
    pub replacement: String,
}

#[derive(Args, Debug)]
pub struct SessionRenamePackageArgs {
    pub session_dir: String,
    pub new_name: String,
}

#[derive(Args, Debug)]
pub struct SessionReplaceSuiteArgs {
    pub session_dir: String,
    pub new_suite: String,
}

#[derive(Args, Debug)]
pub struct SessionReversionArgs {
    pub session_dir: String,
    pub new_version: String,
    /// Also rewrite version constraints in Depends / Pre-Depends / etc.
    #[arg(long = "update-deps", default_value_t = false)]
    pub update_deps: bool,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)] // clap-derive arg structs are wide by nature
pub enum VerifyCommand {
    /// Compare a built .deb's metadata against expected values
    Content(VerifyContentArgs),
    /// Verify the signature on a .deb using debsig-verify
    Signature(VerifySignatureArgs),
}

#[derive(Subcommand, Debug)]
pub enum LookupCommand {
    /// Print the signing-key id embedded in a .deb
    #[command(name = "sign-key")]
    SignKey(LookupSignKeyArgs),
}

#[derive(Args, Debug)]
pub struct BuildArgs {
    /// Path to the directory where the build artifacts are stored
    #[arg(long = "build-dir")]
    pub build_dir: String,

    /// Path to the directory where the output debian package will be stored
    #[arg(long = "output-dir")]
    pub output_dir: String,

    /// Clean the build directory after building
    #[arg(long = "clean", default_value_t = false)]
    pub clean: bool,

    /// JSON file with default values for non-package-specific fields
    #[arg(long = "defaults-file")]
    pub defaults_file: Option<String>,

    /// Name of the debian package
    #[arg(long = "package-name")]
    pub package_name: String,

    /// Version of the debian package
    #[arg(long = "version")]
    pub version: String,

    /// Suite of the debian package (release/stable/unstable/...)
    #[arg(long = "suite")]
    pub suite: String,

    /// Codename of the debian package (focal/bullseye/...)
    #[arg(long = "codename")]
    pub codename: String,

    #[command(flatten)]
    pub metadata: OptionalMetadataArgs,

    /// Enable debug output
    #[arg(long = "debug", default_value_t = false)]
    pub debug: bool,
}

#[derive(Args, Debug)]
pub struct VerifyContentArgs {
    /// Path to the .deb to verify
    #[arg(long = "deb")]
    pub deb: String,

    /// JSON file with default values for fields
    #[arg(long = "defaults-file")]
    pub defaults_file: Option<String>,

    /// Suite (optional for verify)
    #[arg(long = "suite")]
    pub suite: Option<String>,

    /// Codename (optional for verify)
    #[arg(long = "codename")]
    pub codename: Option<String>,

    #[command(flatten)]
    pub metadata: OptionalMetadataArgs,

    /// Enable debug output
    #[arg(long = "debug", default_value_t = false)]
    pub debug: bool,
}

#[derive(Args, Debug)]
pub struct SignArgs {
    /// Path to the .deb to sign
    #[arg(long = "deb")]
    pub deb: String,

    /// Key id to sign with (resolved by gpg)
    #[arg(long = "key")]
    pub key: String,

    /// Enable debug output
    #[arg(long = "debug", default_value_t = false)]
    pub debug: bool,
}

#[derive(Args, Debug)]
pub struct VerifySignatureArgs {
    /// Path to the .deb whose signature should be verified
    pub deb: String,

    /// Optional public key file (path or http(s) URL).
    /// If omitted, debsig-verify uses the system keyring.
    #[arg(long = "key")]
    pub key: Option<String>,

    /// Enable debug output
    #[arg(long = "debug", default_value_t = false)]
    pub debug: bool,
}

#[derive(Args, Debug)]
pub struct LookupSignKeyArgs {
    /// Path to the .deb to inspect
    pub deb: String,

    /// Enable debug output
    #[arg(long = "debug", default_value_t = false)]
    pub debug: bool,
}

/// Fields that can come from either the CLI or the defaults file.
/// All optional here; resolution against defaults happens later.
#[derive(Args, Debug, Default, Clone)]
pub struct OptionalMetadataArgs {
    /// Comma-separated dependencies
    #[arg(long = "depends")]
    pub depends: Option<String>,

    #[arg(long = "suggested-depends")]
    pub suggested_depends: Option<String>,

    #[arg(long = "recommended-depends")]
    pub recommended_depends: Option<String>,

    #[arg(long = "pre-depends")]
    pub pre_depends: Option<String>,

    #[arg(long = "conflicts")]
    pub conflicts: Option<String>,

    #[arg(long = "replaces")]
    pub replaces: Option<String>,

    #[arg(long = "provides")]
    pub provides: Option<String>,

    #[arg(long = "vendor")]
    pub vendor: Option<String>,

    #[arg(long = "authors")]
    pub authors: Option<String>,

    #[arg(long = "maintainer")]
    pub maintainer: Option<String>,

    /// Package description.  Alias kept for parity with the OCaml CLI.
    #[arg(long = "description", visible_alias = "package-description")]
    pub description: Option<String>,

    #[arg(long = "section")]
    pub section: Option<String>,

    #[arg(long = "priority")]
    pub priority: Option<String>,

    #[arg(long = "homepage")]
    pub homepage: Option<String>,

    #[arg(long = "installed-size")]
    pub installed_size: Option<String>,

    #[arg(long = "source")]
    pub source: Option<String>,

    #[arg(long = "architecture")]
    pub architecture: Option<String>,

    #[arg(long = "license")]
    pub license: Option<String>,

    #[arg(long = "githash")]
    pub githash: Option<String>,

    #[arg(long = "buildurl")]
    pub buildurl: Option<String>,
}
