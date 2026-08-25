//! `release-manager publish` — put already-built `.deb` files into an APT
//! repository, unchanged, then check they are really there.
//!
//! This is the direct-publish path: build, test, publish. The package that
//! reaches the repository is byte for byte the package the build produced.
//!
//! Which also means it is the package that *stays* there. Before uploading,
//! each package is compared against what the repository already holds at that
//! name, version and architecture: absent gets uploaded, identical gets
//! skipped, and different bytes at a version that is already published fail
//! the command. So a re-run over the same folder — the retry after a partial
//! upload — converges and exits 0, while a second, different build at the
//! same version is refused rather than silently replacing what users install.
//! `--force` is the way to say the repository copy is the wrong one.
//!
//! "Identical" is decided from the manifest *and* the pool object it names,
//! never from the manifest alone: deb-s3 writes the index before the `.deb`,
//! so an interrupted run leaves a repository that lists a package it does not
//! have, and that is precisely the state a retry exists to repair.
//!
//! `publish-from-cache` is the older flow, kept as a fallback. It is made for
//! the case where one build is re-versioned on its way to a channel, so it
//! demands a source/target version pair and reaches into the CI cache by
//! Buildkite build id. When the build already carries its final version and
//! its final suite — one commit, one version, one channel — that is all
//! ceremony, and setting the two versions equal to switch the rewrite off
//! hides the intent instead of stating it.
//!
//! Anything that must change a package is `reversion`, which runs before this
//! and is visible in the pipeline as its own step.

use colored::*;
use std::path::{Path, PathBuf};

use crate::artifacts::parse_string_list;
use crate::cdn::{bucket_name, invalidate_cloudfront};
use crate::cli::PublishArgs;
use crate::errors::{ManagerError, ManagerResult};
use crate::process::{CommandExecutor, RealExecutor, S3Config};
use crate::utils::print_operation_info;

const S3_REGION: &str = "us-west-2";

/// One codename's worth of work: the folder to upload and the packages in it.
#[derive(Debug, PartialEq, Eq)]
pub struct CodenameBatch {
    pub codename: String,
    pub debs: Vec<PathBuf>,
}

pub async fn execute(args: PublishArgs) -> ManagerResult<()> {
    let exec = RealExecutor;
    let s3 = S3Config {
        endpoint: args.s3_endpoint.clone(),
        force_path_style: args.s3_force_path_style,
        // Credentials are deliberately not a command-line option: deb-s3
        // already reads them from the environment, which is where CI keeps
        // them, and an argument would put a secret in the process list.
        access_key_id: None,
        secret_access_key: None,
    };
    execute_with(args, &exec, &s3).await
}

/// Same as [`execute`] with the external-process and S3-endpoint dependencies
/// injected, so tests can drive it with a `MockCommandExecutor` or run it
/// against MinIO.
pub async fn execute_with(
    args: PublishArgs,
    exec: &dyn CommandExecutor,
    s3: &S3Config,
) -> ManagerResult<()> {
    let wanted = args.codenames.as_deref().map(parse_string_list);
    let batches = collect_batches(Path::new(&args.source_folder), wanted.as_deref())?;

    print_operation_info(
        "Uploading debian packages",
        &[
            ("Source folder", args.source_folder.as_str()),
            ("Repository", args.debian_repo.as_str()),
            ("Channel", args.channel.as_str()),
            (
                "Codenames",
                &batches
                    .iter()
                    .map(|b| b.codename.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            (
                "Signing",
                args.debian_sign_key.as_deref().unwrap_or("unsigned"),
            ),
            ("Mode", if args.dry_run { "dry run" } else { "upload" }),
        ],
    );

    let bucket = bucket_name(&args.debian_repo);
    let mut uploaded = 0usize;
    let mut skipped = 0usize;

    // Plan first, upload second. Every codename is checked against the
    // repository before any of them is touched, so a package that would
    // overwrite different bytes in the last codename fails the command
    // before the first codename has been published.
    let mut plans: Vec<(&CodenameBatch, Vec<PathBuf>)> = Vec::new();
    for batch in &batches {
        println!(" 🔎 {} ({} package(s))", batch.codename, batch.debs.len());
        for deb in &batch.debs {
            println!(
                "    • {}",
                deb.file_name().and_then(|s| s.to_str()).unwrap_or("?")
            );
        }

        if args.dry_run {
            // A dry run stays offline. It is the one mode that is guaranteed
            // to need no repository access at all, and turning a "show me
            // what you would do" flag into something that needs credentials
            // is a bigger change than the extra reporting is worth.
            println!("    ⏩ Dry run — nothing uploaded");
            println!();
            continue;
        }

        // What does the repository already hold? deb-s3's --fail-if-exists
        // does not answer this: it only refuses a same name+version under a
        // *different* pool filename, so a re-publish of the same file name
        // replaces the pool object and reports success. The guard therefore
        // has to live here, before the upload.
        let to_upload: Vec<PathBuf> = if args.force {
            batch.debs.clone()
        } else {
            preflight(
                exec,
                s3,
                &bucket,
                &batch.codename,
                &args.channel,
                &batch.debs,
            )?
        };
        skipped += batch.debs.len() - to_upload.len();
        // Paired rather than parallel vectors: a codename can only reach the
        // upload loop with the plan that was made for it.
        plans.push((batch, to_upload));
        println!();
    }

    if args.dry_run {
        println!(
            "{}",
            " ✅  Dry run complete — nothing was uploaded.".green()
        );
        println!();
        return Ok(());
    }

    for (batch, to_upload) in &plans {
        println!(" 📦 {}", batch.codename);

        if to_upload.is_empty() {
            // Not a no-op: --verify and the CDN invalidation below still run,
            // so a re-run over an already-published folder still proves the
            // end state instead of assuming it. Calling `deb-s3 upload` with
            // no files would fail ("You must specify at least one file to
            // upload") and would have nothing to add to the manifest anyway:
            // for every package in *this folder*, the pool object was just
            // confirmed to hold the bytes the index describes.
            println!("    ⏩ Every package is already published, unchanged — nothing to upload");
        } else {
            // One deb-s3 call per codename rather than one per package: deb-s3
            // takes the repository lock for the whole invocation, so uploading
            // package by package would take and release it N times and rewrite
            // the manifest N times.
            let argv = upload_argv(
                &bucket,
                &batch.codename,
                &args.channel,
                to_upload,
                args.debian_sign_key.as_deref(),
                args.force,
                s3,
            );
            let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = exec
                .run("deb-s3", &argv_refs)
                .map_err(|e| ManagerError::CommandFailed(format!("deb-s3 upload: {}", e)))?;

            for line in out.stdout.lines() {
                println!("    {}", line);
            }
            for line in out.stderr.lines() {
                eprintln!("    {}", line);
            }
            if !out.is_success() {
                return Err(ManagerError::CommandFailed(format!(
                    "deb-s3 upload failed for {} (exit {}): {}",
                    batch.codename,
                    out.status,
                    out.stderr.trim()
                )));
            }
            uploaded += to_upload.len();
            println!("    ✅ Uploaded");
        }

        if args.verify {
            verify_present(
                exec,
                s3,
                &bucket,
                &batch.codename,
                &args.channel,
                &batch.debs,
                args.verify_attempts,
                args.verify_interval_secs,
            )
            .await?;
        }

        if args.skip_cache_invalidation {
            println!("    ⏩ Skipping CDN invalidation as requested");
        } else {
            println!("    🗑️  Invalidating CDN cache...");
            invalidate_cloudfront(exec, &args.debian_repo, &batch.codename)?;
        }

        println!();
    }

    let already = if skipped > 0 {
        format!(" {} package(s) were already published, unchanged.", skipped)
    } else {
        String::new()
    };
    println!(
        "{}",
        format!(
            " ✅  {} package(s) uploaded to {}/{}.{}",
            uploaded, args.debian_repo, args.channel, already
        )
        .green()
    );
    println!();
    Ok(())
}

/// Read `{source_folder}/{codename}/*.deb` into one batch per codename.
///
/// An empty codename folder is an error rather than a silent skip: a pipeline
/// that uploads nothing at all must not report success, because the usual
/// cause is a cache read that produced no files.
pub fn collect_batches(
    source: &Path,
    wanted: Option<&[String]>,
) -> ManagerResult<Vec<CodenameBatch>> {
    if !source.is_dir() {
        return Err(ManagerError::ValidationError(format!(
            "Source folder does not exist: {}",
            source.display()
        )));
    }

    let mut batches: Vec<CodenameBatch> = Vec::new();

    let mut entries: Vec<PathBuf> = std::fs::read_dir(source)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();

    for dir in entries {
        let codename = match dir.file_name().and_then(|s| s.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        if let Some(list) = wanted {
            if !list.iter().any(|w| w == &codename) {
                continue;
            }
        }

        let mut debs: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("deb"))
            .collect();
        debs.sort();

        if debs.is_empty() {
            return Err(ManagerError::ValidationError(format!(
                "No .deb files in {} — refusing to report a successful upload of nothing",
                dir.display()
            )));
        }
        batches.push(CodenameBatch { codename, debs });
    }

    if batches.is_empty() {
        return Err(ManagerError::ValidationError(match wanted {
            Some(list) => format!(
                "No codename folders matching [{}] under {}",
                list.join(","),
                source.display()
            ),
            None => format!("No codename folders under {}", source.display()),
        }));
    }

    Ok(batches)
}

/// Build the `deb-s3 upload` argv for one codename.
///
/// `--component` and `--suite` both get the channel, which is what makes a
/// package land at `dists/{codename}/{channel}/`. No `--arch` is passed:
/// deb-s3 falls back to each package's own `Architecture` field, so one call
/// handles a mixed-architecture folder correctly.
pub fn upload_argv(
    bucket: &str,
    codename: &str,
    channel: &str,
    debs: &[PathBuf],
    sign_key: Option<&str>,
    force: bool,
    s3: &S3Config,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "upload".to_string(),
        format!("--bucket={}", bucket),
        format!("--s3-region={}", S3_REGION),
        "--codename".to_string(),
        codename.to_string(),
        "--component".to_string(),
        channel.to_string(),
        "--suite".to_string(),
        channel.to_string(),
        "--preserve-versions".to_string(),
        "--lock".to_string(),
        "--cache-control=no-store,no-cache,must-revalidate".to_string(),
    ];
    if !force {
        // A backstop, not the guard. deb-s3's --fail-if-exists only raises
        // when a package with the same name and version is already in the
        // manifest under a *different* pool file name; a re-publish of the
        // same file name falls through and replaces the pool object. The
        // check that actually prevents that is [`preflight`], which runs
        // before this call. Keeping the flag costs nothing and still catches
        // the cases it does cover (an epoch appearing in `Version:`, a
        // hand-placed pool path, a leftover `.deb-s3-temp` from a crashed
        // run whose bytes differ).
        argv.push("--fail-if-exists".to_string());
    }
    if let Some(key) = sign_key {
        argv.push("--sign".to_string());
        argv.push(key.to_string());
    }
    s3.append_args(&mut argv);
    for deb in debs {
        argv.push(deb.to_string_lossy().into_owned());
    }
    argv
}

/// A package as the repository will know it, read from its file name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackageRef {
    pub name: String,
    pub version: String,
    pub arch: String,
}

impl std::fmt::Display for PackageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {} {}", self.name, self.version, self.arch)
    }
}

/// Read `{name}_{version}_{arch}.deb` file names into package references.
///
/// A name that does not have the three parts is an error rather than a skip:
/// it would otherwise drop out of the verification silently, and a package
/// nobody checked is exactly what this is here to catch.
pub fn package_refs(debs: &[PathBuf]) -> ManagerResult<Vec<PackageRef>> {
    debs.iter()
        .map(|deb| {
            let base = deb.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
                ManagerError::ValidationError(format!("Bad path: {}", deb.display()))
            })?;
            let stem = base.trim_end_matches(".deb");
            // From the right: a package name may contain underscores, the
            // version and the architecture may not.
            let parts: Vec<&str> = stem.rsplitn(3, '_').collect(); // [arch, version, name]
            if parts.len() != 3 {
                return Err(ManagerError::ValidationError(format!(
                    "Cannot read name/version/arch from {} — cannot verify it was published",
                    base
                )));
            }
            Ok(PackageRef {
                name: parts[2].to_string(),
                version: parts[1].to_string(),
                arch: parts[0].to_string(),
            })
        })
        .collect()
}

/// The `deb-s3 exist` argv for a single package.
///
/// One package per call on purpose. Two builds of `deb-s3` are in use and
/// they disagree about this subcommand: the pinned fork has `exist`, which
/// takes the package names as one space-joined argument, while the Debian
/// `ruby-deb-s3` gem has `exists`, which takes them as separate arguments.
/// A single package is the one shape both accept — Thor resolves the `exist`
/// prefix to `exists` on the gem — so the verification behaves the same
/// wherever it runs. `exist` only reads the manifest and takes no repository
/// lock, so the extra calls are cheap, and the retry below re-asks only about
/// packages that have not turned up yet.
pub fn exist_argv(
    bucket: &str,
    codename: &str,
    channel: &str,
    package: &PackageRef,
    s3: &S3Config,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "exist".to_string(),
        package.name.clone(),
        package.version.clone(),
        package.arch.clone(),
        format!("--bucket={}", bucket),
        format!("--s3-region={}", S3_REGION),
        "--codename".to_string(),
        codename.to_string(),
        "--component".to_string(),
        channel.to_string(),
    ];
    s3.append_args(&mut argv);
    argv
}

/// Read one package's verdict out of `deb-s3 exist` output.
///
/// `Some(true)` found, `Some(false)` missing, `None` no verdict given.
///
/// The exit status cannot be used: the fork always exits 0 and states the
/// answer on stdout. The two builds also word it differently — the fork
/// prints `name : Found` and the gem prints `>> name version arch: Found` —
/// so the line is matched on its trailing verdict and on the package name,
/// not on an exact format.
///
/// `None` is deliberately distinct from `Some(false)`. "The repository says
/// this package is absent" and "the output could not be understood" are
/// different facts, and only the first is worth retrying; treating the second
/// as `Found` would let a publish report success it never checked.
pub fn verdict_from_exist_output(stdout: &str, package: &PackageRef) -> Option<bool> {
    for line in stdout.lines() {
        let line = line.trim().trim_start_matches(">>").trim();
        let (subject, verdict) = match line.rsplit_once(':') {
            Some(parts) => parts,
            None => continue,
        };
        let found = match verdict.trim().to_ascii_lowercase().as_str() {
            "found" => true,
            "missing" => false,
            _ => continue,
        };
        // The subject is `name` on the fork and `name version arch` on the
        // gem; either way the first token is the package name.
        if subject.split_whitespace().next() == Some(package.name.as_str()) {
            return Some(found);
        }
    }
    None
}

/// The `deb-s3 show` argv for a single package.
///
/// Same shape as [`exist_argv`]: the name, version and architecture are
/// positional, so no `--arch` flag is passed. `show` reads the manifest
/// through the S3 API rather than over the CDN, so unlike an HTTPS fetch of
/// `dists/…/Packages` it cannot serve a stale answer, and it takes no
/// repository lock.
///
/// `show` and `exist` share their lookup — both read
/// `dists/{codename}/{component}/binary-{arch}/Packages` and match on name
/// and full version — so this reaches exactly the packages `--verify`
/// already reaches, including `Architecture: all` ones. deb-s3 writes a
/// `binary-all` manifest of its own alongside merging those packages into
/// each real architecture, and a `show … all` finds them there.
pub fn show_argv(
    bucket: &str,
    codename: &str,
    channel: &str,
    package: &PackageRef,
    s3: &S3Config,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "show".to_string(),
        package.name.clone(),
        package.version.clone(),
        package.arch.clone(),
        format!("--bucket={}", bucket),
        format!("--s3-region={}", S3_REGION),
        "--codename".to_string(),
        codename.to_string(),
        "--component".to_string(),
        channel.to_string(),
    ];
    s3.append_args(&mut argv);
    argv
}

/// Read one field out of a `deb-s3 show` stanza.
///
/// Only an unindented field counts. A `Description:` continuation line is
/// indented by one space, so a description that happens to quote a field name
/// cannot be mistaken for the field itself. The first match wins, as it does
/// in a Debian control paragraph.
fn field_from_show_output<'a>(stdout: &'a str, field: &str) -> Option<&'a str> {
    for line in stdout.lines() {
        if line.starts_with(char::is_whitespace) {
            continue; // continuation of the previous field
        }
        let (key, value) = match line.split_once(':') {
            Some(parts) => parts,
            None => continue,
        };
        if key.trim().eq_ignore_ascii_case(field) {
            return Some(value.trim());
        }
    }
    None
}

/// Read the `SHA256:` field out of a `deb-s3 show` stanza.
///
/// SHA256 rather than the `MD5sum:` line next to it because SHA256 is what
/// apt verifies, and it is the digest of the `.deb` itself — deb-s3 takes it
/// with `Digest::SHA2.file` when it parses the package — so it can be
/// compared straight against a digest of the local file.
///
/// `None` means no usable field was found. As with
/// [`verdict_from_exist_output`], that is deliberately not the same fact as
/// "the package is absent": an output we cannot read must fail the command,
/// never be assumed to be a mismatch or a match.
pub fn sha256_from_show_output(stdout: &str) -> Option<String> {
    hex_field(stdout, "SHA256", 64)
}

/// Read the `MD5sum:` field, which is what an S3 `HEAD` can be compared
/// against without downloading anything.
pub fn md5_from_show_output(stdout: &str) -> Option<String> {
    hex_field(stdout, "MD5sum", 32)
}

/// A stanza field that has to be a hex digest of a given length. A field that
/// is there but is not a digest is not an answer, so it reads as `None`.
fn hex_field(stdout: &str, field: &str, len: usize) -> Option<String> {
    let digest = field_from_show_output(stdout, field)?.to_ascii_lowercase();
    if digest.len() == len && digest.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(digest)
    } else {
        None
    }
}

/// Read the `Size:` field — the length of the `.deb`, in bytes.
pub fn size_from_show_output(stdout: &str) -> Option<u64> {
    field_from_show_output(stdout, "Size")?.parse().ok()
}

/// Read the `Filename:` field out of a `deb-s3 show` stanza — the pool key
/// the manifest says the package lives at.
///
/// deb-s3 percent-escapes that field when it *writes* the manifest and
/// un-escapes it when it reads one back (`package.rb:254`), so what `show`
/// prints is the S3 key itself, which is what the pool objects are stored
/// under (`manifest.rb:107` passes the un-escaped `url_filename`).
///
/// Not quite lossless: `s3_escape` leaves `+` alone (`utils.rb:52-56`) while
/// the `CGI.unescape` on the way back turns it into a space, so a `.deb` with
/// a `+` in its name would yield a key that heads-object as missing. No Mina
/// version carries one, and the failure is in the safe direction — the
/// package is uploaded again rather than wrongly skipped.
pub fn pool_key_from_show_output(stdout: &str) -> Option<String> {
    match field_from_show_output(stdout, "Filename") {
        Some(path) if !path.is_empty() => Some(path.to_string()),
        _ => None,
    }
}

/// The `aws s3api head-object` argv for one pool object.
///
/// `aws` rather than `deb-s3`: deb-s3 has no per-package "is the file really
/// there" subcommand, and this repository already depends on the AWS CLI for
/// the CloudFront invalidation a few lines later. Credentials are not passed
/// as arguments — `aws` reads them from the environment, exactly as `deb-s3`
/// does, which is where CI keeps them and keeps them out of the process list.
///
/// `S3Config::force_path_style` has no argv equivalent in the AWS CLI, so it
/// is not forwarded. It does not need to be for either configuration that
/// matters: the CLI already addresses a dotted bucket
/// (`stable.apt.packages.minaprotocol.com`) and an IP-address endpoint (a
/// local MinIO) path-style on its own. An S3-compatible endpoint reached by
/// *host name* with an undotted bucket is the gap; there the check fails,
/// which means every package is uploaded again rather than skipped.
pub fn head_object_argv(bucket: &str, key: &str, s3: &S3Config) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "s3api".to_string(),
        "head-object".to_string(),
        "--bucket".to_string(),
        bucket.to_string(),
        "--key".to_string(),
        key.to_string(),
        "--region".to_string(),
        S3_REGION.to_string(),
    ];
    if let Some(endpoint) = &s3.endpoint {
        argv.push("--endpoint-url".to_string());
        argv.push(endpoint.clone());
    }
    argv
}

/// SHA256 of a local file, streamed rather than read into memory: a `.deb`
/// can be hundreds of megabytes.
fn local_sha256(path: &Path) -> ManagerResult<String> {
    use sha2::{Digest, Sha256};

    let mut file = std::fs::File::open(path).map_err(|e| {
        ManagerError::ValidationError(format!("Cannot read {}: {}", path.display(), e))
    })?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| {
        ManagerError::ValidationError(format!("Cannot read {}: {}", path.display(), e))
    })?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// What the repository holds for one package we are about to upload.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RepoState {
    /// Not published at this name+version+arch. Upload it.
    Absent,
    /// Published, byte for byte the file we hold, and the `.deb` it points at
    /// is really in the pool. Skip it.
    Identical,
    /// The manifest lists our exact bytes, but the pool object it names could
    /// not be confirmed. Upload it again — see [`pool_object_present`].
    Incomplete { reason: String },
    /// Published at the same version as *different* bytes. Refuse.
    Differs {
        repo_sha256: String,
        local_sha256: String,
    },
}

/// Whether a failed `deb-s3 show` failed because the package is not there.
///
/// The pinned fork answers an absent package with `error "No such package
/// found."` (`cli.rb:647-650`), which is `!! No such package found.` on
/// **stderr** and exit 1. Every other non-zero exit — no credentials, no
/// bucket, no `show` subcommand — is a real failure and must not be read as
/// "absent", because "absent" means "go ahead and upload". Only stderr is
/// examined, so a package whose description quotes the phrase cannot vote.
///
/// This depends on `--quiet` never being passed: `error` prints the message
/// `unless options[:quiet]`, so under `-q` an absent package would exit 1
/// with an empty stderr and every first publish would become a hard error.
/// Neither [`show_argv`] nor [`S3Config::append_args`] passes it.
fn says_no_such_package(out: &crate::process::CommandOutput) -> bool {
    out.stderr.to_ascii_lowercase().contains("no such package")
}

/// Is the `.deb` a manifest stanza names really in the bucket, and is it the
/// one the stanza describes?
///
/// `Ok(())` if it is; `Err(reason)` if it is not, or if we could not find
/// out. Both non-answers lead to the same place — upload the package again —
/// so this deliberately does not fail the command. Re-uploading bytes that
/// are already there is safe and idempotent; it is what this command did
/// before the skip existed. Skipping without an answer is not.
///
/// This is what makes a skip mean something. deb-s3 writes the manifest
/// *before* the pool objects (`cli.rb:254-283`: `write_manifests_to_s3`, then
/// `release.write_to_s3`, then the lock is released, then
/// `write_packages_to_s3` as `.deb-s3-temp`, then `s3_finish_atomic_store`
/// copies each temp to its real key). A run killed anywhere in that tail —
/// the long part, uploading gigabytes, and the part that is no longer under
/// the lock — leaves a `Packages` index advertising a package, with the right
/// SHA256, whose `.deb` is not there, or is still the *previous* build. The
/// first case 404s on install; the second is worse, because apt reports it as
/// a hash mismatch. Trusting the manifest alone would make the retry skip the
/// upload forever and report success over either one.
///
/// Nothing is downloaded to tell them apart. `HEAD` returns the object's
/// length, and deb-s3 stamps the file's MD5 into the object's metadata when
/// it stores it (`utils.rb:93`), which the temp-to-final `copy_object`
/// preserves; the manifest stanza carries both. For an object put by
/// something other than deb-s3, `ETag` is the same digest as long as it was
/// not a multipart upload.
fn pool_object_matches(
    exec: &dyn CommandExecutor,
    s3: &S3Config,
    bucket: &str,
    key: &str,
    expected_size: Option<u64>,
    expected_md5: Option<&str>,
) -> Result<(), String> {
    let argv = head_object_argv(bucket, key, s3);
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = match exec.run("aws", &argv_refs) {
        Ok(out) if out.is_success() => out,
        Ok(out) => {
            return Err(format!(
                "{} is not readable in the bucket (aws s3api head-object exit {}: {})",
                key,
                out.status,
                blank_if_empty(&out.stderr)
            ))
        }
        Err(e) => return Err(format!("could not check {} in the bucket: {}", key, e)),
    };

    let head: serde_json::Value = serde_json::from_str(&out.stdout)
        .map_err(|e| format!("could not read the head-object answer for {}: {}", key, e))?;

    let actual_size = head.get("ContentLength").and_then(|v| v.as_u64());
    if let (Some(want), Some(got)) = (expected_size, actual_size) {
        if want != got {
            return Err(format!(
                "{} holds {} bytes but the index says {}",
                key, got, want
            ));
        }
    }

    // The metadata deb-s3 wrote first; ETag as a fallback, and only when it
    // is a plain MD5 — a multipart upload's ETag is a digest of digests with
    // a `-partcount` suffix and cannot be compared with anything here.
    let actual_md5 = head
        .get("Metadata")
        .and_then(|m| m.get("md5"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase())
        .or_else(|| {
            head.get("ETag")
                .and_then(|v| v.as_str())
                .map(|s| s.trim_matches('"').to_ascii_lowercase())
                .filter(|e| e.len() == 32 && e.chars().all(|c| c.is_ascii_hexdigit()))
        });

    match (expected_md5, actual_md5.as_deref()) {
        (Some(want), Some(got)) if want == got => Ok(()),
        (Some(_), Some(got)) => Err(format!(
            "{} holds different bytes than the index describes (MD5 {})",
            key, got
        )),
        _ => Err(format!(
            "{} could not be matched against the index (no comparable MD5)",
            key
        )),
    }
}

/// Ask the repository what it currently holds for one package.
fn classify_one(
    exec: &dyn CommandExecutor,
    s3: &S3Config,
    bucket: &str,
    codename: &str,
    channel: &str,
    deb: &Path,
    package: &PackageRef,
) -> ManagerResult<RepoState> {
    let argv = show_argv(bucket, codename, channel, package, s3);
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = exec
        .run("deb-s3", &argv_refs)
        .map_err(|e| ManagerError::CommandFailed(format!("deb-s3 show: {}", e)))?;

    if !out.is_success() {
        if says_no_such_package(&out) {
            return Ok(RepoState::Absent);
        }
        return Err(ManagerError::CommandFailed(format!(
            "cannot tell what the repository holds for {} (`deb-s3 show` exit {}).\n  \
             stdout: {}\n  stderr: {}\n\
             Only `No such package found.` means the package is absent; anything else \
             is a failure to read the repository, and uploading over an answer we did \
             not get is what this check exists to prevent.",
            package,
            out.status,
            blank_if_empty(&out.stdout),
            blank_if_empty(&out.stderr)
        )));
    }

    let unreadable = |what: &str| {
        ManagerError::CommandFailed(format!(
            "`deb-s3 show` reported {} as published but gave no readable {}.\n  \
             stdout: {}\n  stderr: {}\n\
             Pass --force to publish over it anyway.",
            package,
            what,
            blank_if_empty(&out.stdout),
            blank_if_empty(&out.stderr)
        ))
    };

    let repo_sha256 = sha256_from_show_output(&out.stdout).ok_or_else(|| unreadable("SHA256"))?;

    // Only now is the local digest worth computing: a first publish — the
    // ordinary case — pays for one `show` per package and nothing else.
    let local_sha256 = local_sha256(deb)?;
    if repo_sha256 != local_sha256 {
        return Ok(RepoState::Differs {
            repo_sha256,
            local_sha256,
        });
    }

    // The manifest agrees with the file we hold. That still says nothing
    // about the `.deb` itself being in the pool.
    //
    // A stanza we cannot read a `Filename:` out of is *not* fatal here, and
    // that is the difference between this and the SHA256 above: the digests
    // have already matched, so uploading again is known to be safe, and
    // failing the run would leave the operator choosing between a stuck
    // pipeline and a --force that switches the guard off for every package.
    let key = match pool_key_from_show_output(&out.stdout) {
        Some(key) => key,
        None => {
            return Ok(RepoState::Incomplete {
                reason: "the index entry names no Filename to check".to_string(),
            })
        }
    };
    match pool_object_matches(
        exec,
        s3,
        bucket,
        &key,
        size_from_show_output(&out.stdout),
        md5_from_show_output(&out.stdout).as_deref(),
    ) {
        Ok(()) => Ok(RepoState::Identical),
        Err(reason) => Ok(RepoState::Incomplete { reason }),
    }
}

/// Narrow a batch to the packages that actually need uploading, and refuse
/// the publish outright if any of them would replace different bytes at a
/// version that is already published.
///
/// Nothing is skipped on the strength of the manifest alone — see
/// [`pool_object_present`]. A skip means the index lists our exact bytes
/// *and* the `.deb` is really in the pool, which in deb-s3's write order also
/// means the `Release` file that covers it was written.
///
/// The read happens outside deb-s3's repository lock. That is deliberate and
/// sufficient: the check guards a re-run of the same build, not two different
/// builds racing each other, and losing that race degrades to the behaviour
/// this command had before the check existed.
fn preflight(
    exec: &dyn CommandExecutor,
    s3: &S3Config,
    bucket: &str,
    codename: &str,
    channel: &str,
    debs: &[PathBuf],
) -> ManagerResult<Vec<PathBuf>> {
    // A name we cannot read is an error here for the same reason it is one in
    // `package_refs`: we cannot ask the repository about a package we cannot
    // name, and uploading it unasked is exactly the unguarded overwrite this
    // is here to prevent.
    //
    // The version comes from the file name and so carries no epoch, while
    // deb-s3 matches on `full_version`, which does. Mina packages have no
    // epoch, so the two agree; one that did would read as forever absent.
    let packages = package_refs(debs)?;

    debug_assert_eq!(debs.len(), packages.len(), "one PackageRef per .deb");
    let mut to_upload: Vec<PathBuf> = Vec::new();
    for (deb, package) in debs.iter().zip(packages.iter()) {
        let base = deb.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        match classify_one(exec, s3, bucket, codename, channel, deb, package)? {
            RepoState::Absent => to_upload.push(deb.clone()),
            RepoState::Incomplete { reason } => {
                println!("    ↻ {} is listed but {} — uploading again", base, reason);
                to_upload.push(deb.clone());
            }
            // An `Architecture: all` package is never skipped. deb-s3 merges
            // those into every architecture manifest that exists *at the time
            // of the upload* (`cli.rb:229-243`), so skipping one leaves an
            // architecture that has appeared since it was first published
            // without it. They are the small config packages, so re-uploading
            // one costs nothing worth saving.
            RepoState::Identical if package.arch == "all" => {
                println!(
                    "    ↻ {} already published, identical — uploading anyway so every \
                     architecture manifest lists it",
                    base
                );
                to_upload.push(deb.clone());
            }
            RepoState::Identical => {
                println!("    ⏩ {} already published, identical — skipping", base);
            }
            RepoState::Differs {
                repo_sha256,
                local_sha256,
            } => {
                return Err(ManagerError::ValidationError(format!(
                    "{} is already published in {}/{} at this version with different contents\n  \
                     repository SHA256: {}\n  \
                     local      SHA256: {}\n\
                     Publishing different bytes at the same version silently changes what \
                     users install: whoever installed it earlier has one artifact and \
                     whoever installs it now gets another, under the same version string. \
                     Publish under a new version, or pass --force — which switches this \
                     check off for every package in the run, not just this one — if the \
                     repository copy is known to be wrong.",
                    base, codename, channel, repo_sha256, local_sha256
                )));
            }
        }
    }
    Ok(to_upload)
}

/// Render a captured stream for an error message, so an empty one reads as
/// empty rather than as nothing at all.
fn blank_if_empty(stream: &str) -> String {
    let trimmed = stream.trim();
    if trimmed.is_empty() {
        "(empty)".to_string()
    } else {
        trimmed.replace('\n', "\n          ")
    }
}

/// Ask the repository whether each package just published is really listed,
/// and retry while any is still absent.
///
/// The retry is not defensive padding. The bucket sits behind a CDN and the
/// index is rewritten as a whole object, so a read straight after a write can
/// legitimately still serve the previous manifest. Reporting a successful
/// publish for a package that cannot yet be installed is the failure this
/// guards against, so the final attempt fails the command rather than warning.
#[allow(clippy::too_many_arguments)]
async fn verify_present(
    exec: &dyn CommandExecutor,
    s3: &S3Config,
    bucket: &str,
    codename: &str,
    channel: &str,
    debs: &[PathBuf],
    attempts: u32,
    interval_secs: u64,
) -> ManagerResult<()> {
    let mut pending = package_refs(debs)?;
    let total = pending.len();
    println!("    🔍 Verifying {} package(s) are published...", total);

    let attempts = attempts.max(1);
    for attempt in 1..=attempts {
        let mut still_pending: Vec<PackageRef> = Vec::new();

        for package in &pending {
            let argv = exist_argv(bucket, codename, channel, package, s3);
            let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = exec
                .run("deb-s3", &argv_refs)
                .map_err(|e| ManagerError::CommandFailed(format!("deb-s3 exist: {}", e)))?;

            match verdict_from_exist_output(&out.stdout, package) {
                Some(true) => {}
                Some(false) => still_pending.push(package.clone()),
                None => {
                    // Not "absent" — "unreadable". Retrying cannot fix a
                    // shape we do not understand, and carrying on would
                    // report a publish that was never checked.
                    //
                    // Both streams and the exit status go into the message.
                    // The usual cause is a `deb-s3` without the subcommand at
                    // all — the rubygems release has no `exist` — and then
                    // stdout is empty and everything worth reading is the
                    // Thor error on stderr.
                    return Err(ManagerError::CommandFailed(format!(
                        "cannot tell whether {} was published: no Found/Missing verdict for it \
                         in `deb-s3 exist` output (exit {}).\n  stdout: {}\n  stderr: {}\n\
                         Check that `deb-s3` has an `exist` subcommand: the rubygems release \
                         does not, only the pinned fork and the Debian ruby-deb-s3 package do.",
                        package,
                        out.status,
                        blank_if_empty(&out.stdout),
                        blank_if_empty(&out.stderr)
                    )));
                }
            }
        }

        pending = still_pending;
        if pending.is_empty() {
            println!("    ✅ All {} package(s) present in the repository", total);
            return Ok(());
        }

        let names: Vec<String> = pending.iter().map(|p| p.name.clone()).collect();
        if attempt == attempts {
            return Err(ManagerError::CommandFailed(format!(
                "published to {}/{} but {} of {} package(s) are still not listed after {} \
                 attempt(s): {}",
                codename,
                channel,
                pending.len(),
                total,
                attempt,
                names.join(", ")
            )));
        }

        println!(
            "    ⏳ {} package(s) not listed yet ({}); retrying in {}s [{}/{}]",
            pending.len(),
            names.join(", "),
            interval_secs,
            attempt,
            attempts
        );
        tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
    }

    unreachable!("the final attempt returns")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{CommandOutput, MockCommandExecutor};

    fn args_for(dir: &Path) -> PublishArgs {
        PublishArgs {
            source_folder: dir.to_string_lossy().into_owned(),
            debian_repo: "stable.apt.packages.minaprotocol.com".to_string(),
            channel: "stable".to_string(),
            codenames: None,
            debian_sign_key: None,
            force: false,
            verify: false,
            // Tests that do exercise --verify set these low: no test should
            // spend half a minute asleep proving a retry happened.
            verify_attempts: 3,
            verify_interval_secs: 0,
            skip_cache_invalidation: true,
            dry_run: false,
            s3_endpoint: None,
            s3_force_path_style: false,
        }
    }

    fn make_tree(specs: &[(&str, &[&str])]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (codename, debs) in specs {
            let dir = root.path().join(codename);
            std::fs::create_dir_all(&dir).unwrap();
            for deb in *debs {
                std::fs::write(dir.join(deb), b"not-a-real-deb").unwrap();
            }
        }
        root
    }

    /// An executor for which every package is new: the pre-flight `show`
    /// answers the way the pinned fork answers for a package that is not in
    /// the repository, and everything else succeeds quietly.
    fn ok_exec() -> MockCommandExecutor {
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], absent());
        exec.expect("deb-s3", |_| true, CommandOutput::success(""));
        exec
    }

    /// `deb-s3 show` for a package that is not published: exit 1 with the
    /// message on stderr, stdout empty.
    fn absent() -> CommandOutput {
        CommandOutput::failure(1, "!! No such package found.")
    }

    /// The `MD5sum:` and `Size:` that [`published_with`] reports.
    const STANZA_MD5: &str = "24e90aff2c9522aed91b892e83a1024f";
    const STANZA_SIZE: u64 = 736;

    /// `aws s3api head-object` for a pool object that is there and holds the
    /// bytes the index describes — including the `md5` metadata deb-s3 writes
    /// (`utils.rb:93`), as the real CLI prints it.
    fn pool_present() -> CommandOutput {
        pool_holding(STANZA_SIZE, STANZA_MD5)
    }

    /// `aws s3api head-object` for a pool object of a given size and digest.
    fn pool_holding(size: u64, md5: &str) -> CommandOutput {
        CommandOutput::success(format!(
            "{{\n    \"ContentLength\": {},\n    \"ETag\": \"\\\"{}\\\"\",\n    \
             \"Metadata\": {{\n        \"md5\": \"{}\"\n    }}\n}}\n",
            size, md5, md5
        ))
    }

    /// `aws s3api head-object` for a pool object that is not — the shape the
    /// AWS CLI really produces for a missing key.
    fn pool_missing() -> CommandOutput {
        CommandOutput::failure(
            254,
            "An error occurred (404) when calling the HeadObject operation: Not Found",
        )
    }

    /// A digest that is not any fixture's, so "the repository holds something
    /// else" is the same fact in every test that needs it.
    const MISMATCHED_DIGEST: &str =
        "6c0dd772a2b8f4e6c2d0d0f0a9f8e7d6c5b4a3928170615243342516273849fa";

    /// `deb-s3 show` for a published package, as the stanza really comes out.
    fn published_with(sha256: &str) -> CommandOutput {
        CommandOutput::success(format!(
            "Package: mina-devnet\n\
             Version: 4.0.0-abc1234\n\
             Architecture: amd64\n\
             Filename: pool/bullseye/m/mi/mina-devnet_4.0.0-abc1234_amd64.deb\n\
             Size: {}\n\
             SHA1: e917d8da7970bc09f3c1cdf4d54a797f7d8be7b0\n\
             SHA256: {}\n\
             MD5sum: {}\n\
             Description: fixture\n",
            STANZA_SIZE, sha256, STANZA_MD5
        ))
    }

    /// The digest `make_tree` fixtures really have, so a mocked `show` can
    /// claim to hold the same bytes the test wrote to disk.
    fn fixture_sha256() -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(b"not-a-real-deb"))
    }

    /// How many `deb-s3` calls were made for a given subcommand.
    fn subcommand_calls(exec: &MockCommandExecutor, subcommand: &str) -> usize {
        exec.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| {
                c.program == "deb-s3" && c.args.first().map(String::as_str) == Some(subcommand)
            })
            .count()
    }

    #[test]
    fn collect_batches_groups_debs_by_codename_folder() {
        let root = make_tree(&[
            ("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"]),
            (
                "noble",
                &[
                    "mina-devnet_4.0.0-abc1234_amd64.deb",
                    "mina-archive_4.0.0-abc1234_amd64.deb",
                ],
            ),
        ]);

        let batches = collect_batches(root.path(), None).unwrap();

        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].codename, "bullseye");
        assert_eq!(batches[0].debs.len(), 1);
        assert_eq!(batches[1].codename, "noble");
        assert_eq!(batches[1].debs.len(), 2);
    }

    #[test]
    fn collect_batches_honours_an_explicit_codename_list() {
        let root = make_tree(&[
            ("bullseye", &["a_1_amd64.deb"]),
            ("noble", &["a_1_amd64.deb"]),
        ]);
        let wanted = vec!["noble".to_string()];

        let batches = collect_batches(root.path(), Some(&wanted)).unwrap();

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].codename, "noble");
    }

    #[test]
    fn collect_batches_ignores_files_that_are_not_debs() {
        let root = make_tree(&[("bullseye", &["a_1_amd64.deb", "a_1_amd64.deb.sha256"])]);

        let batches = collect_batches(root.path(), None).unwrap();

        assert_eq!(batches[0].debs.len(), 1);
    }

    #[test]
    fn an_empty_codename_folder_is_an_error_not_a_silent_skip() {
        let root = make_tree(&[("bullseye", &[])]);

        let err = collect_batches(root.path(), None).unwrap_err();

        assert!(
            err.to_string().contains("No .deb files"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn a_source_folder_with_no_codenames_is_an_error() {
        let root = tempfile::tempdir().unwrap();

        let err = collect_batches(root.path(), None).unwrap_err();

        assert!(
            err.to_string().contains("No codename folders"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn a_missing_source_folder_is_an_error() {
        let err = collect_batches(Path::new("/definitely/not/here"), None).unwrap_err();

        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn upload_argv_puts_the_channel_in_both_component_and_suite() {
        let argv = upload_argv(
            "b",
            "bullseye",
            "stable",
            &[PathBuf::from("x.deb")],
            None,
            false,
            &S3Config::default(),
        );

        let component = argv.iter().position(|a| a == "--component").unwrap();
        let suite = argv.iter().position(|a| a == "--suite").unwrap();
        assert_eq!(argv[component + 1], "stable");
        assert_eq!(argv[suite + 1], "stable");
    }

    #[test]
    fn upload_argv_never_passes_an_arch_so_deb_s3_reads_it_from_the_package() {
        let argv = upload_argv(
            "b",
            "bullseye",
            "stable",
            &[PathBuf::from("x_1_arm64.deb")],
            None,
            false,
            &S3Config::default(),
        );

        assert!(!argv.iter().any(|a| a == "--arch"), "argv: {:?}", argv);
    }

    #[test]
    fn upload_argv_guards_against_overwrite_unless_force_is_given() {
        let base = upload_argv(
            "b",
            "bullseye",
            "stable",
            &[PathBuf::from("x.deb")],
            None,
            false,
            &S3Config::default(),
        );
        assert!(base.iter().any(|a| a == "--fail-if-exists"));

        let forced = upload_argv(
            "b",
            "bullseye",
            "stable",
            &[PathBuf::from("x.deb")],
            None,
            true,
            &S3Config::default(),
        );
        assert!(!forced.iter().any(|a| a == "--fail-if-exists"));
    }

    #[test]
    fn upload_argv_appends_the_signing_key_when_one_is_given() {
        let argv = upload_argv(
            "b",
            "bullseye",
            "stable",
            &[PathBuf::from("x.deb")],
            Some("KEYID"),
            false,
            &S3Config::default(),
        );

        let sign = argv.iter().position(|a| a == "--sign").unwrap();
        assert_eq!(argv[sign + 1], "KEYID");
    }

    #[test]
    fn upload_argv_lists_every_deb_of_the_batch() {
        let debs = vec![PathBuf::from("a.deb"), PathBuf::from("b.deb")];

        let argv = upload_argv(
            "b",
            "bullseye",
            "stable",
            &debs,
            None,
            false,
            &S3Config::default(),
        );

        assert!(argv.iter().any(|a| a == "a.deb"));
        assert!(argv.iter().any(|a| a == "b.deb"));
    }

    #[tokio::test]
    async fn upload_makes_one_deb_s3_call_per_codename() {
        let root = make_tree(&[
            ("bullseye", &["a_1_amd64.deb", "b_1_amd64.deb"]),
            ("noble", &["a_1_amd64.deb"]),
        ]);
        let exec = ok_exec();

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(subcommand_calls(&exec, "upload"), 2);
        // One pre-flight read per package, not per codename.
        assert_eq!(subcommand_calls(&exec, "show"), 3);
    }

    #[tokio::test]
    async fn a_dry_run_touches_nothing() {
        let root = make_tree(&[("bullseye", &["a_1_amd64.deb"])]);
        let exec = ok_exec();
        let mut args = args_for(root.path());
        args.dry_run = true;

        execute_with(args, &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(exec.call_count("deb-s3"), 0);
    }

    #[tokio::test]
    async fn a_failed_upload_stops_and_reports_rather_than_continuing() {
        let root = make_tree(&[
            ("bullseye", &["a_1_amd64.deb"]),
            ("noble", &["a_1_amd64.deb"]),
        ]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], absent());
        exec.expect(
            "deb-s3",
            |_| true,
            CommandOutput::failure(1, "Package already exists"),
        );

        let err = execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("deb-s3 upload failed"));
        assert_eq!(
            subcommand_calls(&exec, "upload"),
            1,
            "must not upload the next codename after a failure"
        );
    }

    #[tokio::test]
    async fn verify_runs_only_when_asked_for() {
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);

        let quiet = ok_exec();
        execute_with(args_for(root.path()), &quiet, &S3Config::default())
            .await
            .unwrap();
        assert_eq!(subcommand_calls(&quiet, "exist"), 0);

        let checked = MockCommandExecutor::new();
        checked.expect_args_starting_with("deb-s3", &["show"], absent());
        checked.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));
        checked.expect_args_starting_with(
            "deb-s3",
            &["exist"],
            CommandOutput::success("mina-devnet : Found"),
        );
        let mut args = args_for(root.path());
        args.verify = true;
        execute_with(args, &checked, &S3Config::default())
            .await
            .unwrap();
        let calls = checked.calls.lock().unwrap();
        let order: Vec<&str> = calls.iter().map(|c| c.args[0].as_str()).collect();
        assert_eq!(order, vec!["show", "upload", "exist"]);
    }

    #[test]
    fn package_refs_are_read_from_the_file_name() {
        let debs = vec![
            PathBuf::from("mina-devnet_4.0.0-abc1234_amd64.deb"),
            PathBuf::from("mina-archive-devnet_4.0.0-abc1234_arm64.deb"),
        ];

        let refs = package_refs(&debs).unwrap();

        assert_eq!(refs[0].name, "mina-devnet");
        assert_eq!(refs[0].version, "4.0.0-abc1234");
        assert_eq!(refs[0].arch, "amd64");
        assert_eq!(refs[1].name, "mina-archive-devnet");
        assert_eq!(refs[1].arch, "arm64");
    }

    #[test]
    fn package_refs_keep_underscores_that_belong_to_the_package_name() {
        // From the right: only the last two underscores are separators.
        let debs = vec![PathBuf::from("mina_test_suite_4.0.0-abc1234_amd64.deb")];

        let refs = package_refs(&debs).unwrap();

        assert_eq!(refs[0].name, "mina_test_suite");
        assert_eq!(refs[0].version, "4.0.0-abc1234");
        assert_eq!(refs[0].arch, "amd64");
    }

    #[test]
    fn an_unparsable_file_name_fails_rather_than_going_unverified() {
        let err = package_refs(&[PathBuf::from("garbage.deb")]).unwrap_err();

        assert!(err.to_string().contains("cannot verify"), "{}", err);
    }

    #[test]
    fn exist_argv_asks_about_one_package_so_both_deb_s3_builds_accept_it() {
        let package = PackageRef {
            name: "mina-devnet".to_string(),
            version: "4.0.0-abc1234".to_string(),
            arch: "amd64".to_string(),
        };

        let argv = exist_argv("b", "bullseye", "stable", &package, &S3Config::default());

        assert_eq!(argv[0], "exist");
        assert_eq!(argv[1], "mina-devnet");
        assert_eq!(argv[2], "4.0.0-abc1234");
        assert_eq!(argv[3], "amd64");
    }

    fn a_package(name: &str) -> PackageRef {
        PackageRef {
            name: name.to_string(),
            version: "4.0.0-abc1234".to_string(),
            arch: "amd64".to_string(),
        }
    }

    #[test]
    fn verdict_reads_the_pinned_forks_wording() {
        let out = "mina-devnet : Found";

        assert_eq!(
            verdict_from_exist_output(out, &a_package("mina-devnet")),
            Some(true)
        );
    }

    #[test]
    fn verdict_reads_the_debian_gems_wording() {
        // The gem logs through `log()`, which prefixes `>> `, and names the
        // version and arch as well.
        let out = ">> mina-devnet 4.0.0-abc1234 amd64: Missing";

        assert_eq!(
            verdict_from_exist_output(out, &a_package("mina-devnet")),
            Some(false)
        );
    }

    #[test]
    fn verdict_ignores_progress_lines() {
        let out = ">> Retrieving existing manifests\nmina-devnet : Found";

        assert_eq!(
            verdict_from_exist_output(out, &a_package("mina-devnet")),
            Some(true)
        );
    }

    #[test]
    fn no_verdict_is_not_the_same_as_missing() {
        // A package the output never mentions must read as "cannot tell",
        // never as "found" — otherwise an unrecognised deb-s3 build would
        // let an unchecked publish report success.
        let out = "some-other-package : Found";

        assert_eq!(
            verdict_from_exist_output(out, &a_package("mina-devnet")),
            None
        );
    }

    #[tokio::test]
    async fn an_unreadable_verdict_fails_instead_of_being_retried() {
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], absent());
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));
        exec.expect_args_starting_with(
            "deb-s3",
            &["exist"],
            CommandOutput::success(">> Retrieving existing manifests"),
        );
        let mut args = args_for(root.path());
        args.verify = true;

        let err = execute_with(args, &exec, &S3Config::default())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("cannot tell whether"), "{}", err);
        assert_eq!(
            subcommand_calls(&exec, "exist"),
            1,
            "an unreadable verdict must not be retried"
        );
    }

    #[tokio::test]
    async fn a_deb_s3_without_exist_says_so_instead_of_showing_an_empty_output() {
        // The rubygems release of deb-s3 has no `exist` subcommand, so Thor
        // fails on stderr and stdout is empty. Reporting only stdout left the
        // reader with "output:" and nothing after it.
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], absent());
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));
        exec.expect_args_starting_with(
            "deb-s3",
            &["exist"],
            CommandOutput::failure(1, "Could not find command \"exist\"."),
        );
        let mut args = args_for(root.path());
        args.verify = true;

        let err = execute_with(args, &exec, &S3Config::default())
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("exit 1"), "{}", err);
        assert!(err.contains("stdout: (empty)"), "{}", err);
        assert!(err.contains("Could not find command"), "{}", err);
        assert!(err.contains("rubygems release"), "{}", err);
    }

    #[tokio::test]
    async fn verify_only_re_asks_about_packages_that_are_still_absent() {
        let root = make_tree(&[(
            "bullseye",
            &[
                "a-pkg_4.0.0-abc1234_amd64.deb",
                "b-pkg_4.0.0-abc1234_amd64.deb",
            ],
        )]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], absent());
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));
        exec.expect(
            "deb-s3",
            |a| a.first() == Some(&"exist") && a.get(1) == Some(&"a-pkg"),
            CommandOutput::success("a-pkg : Found"),
        );
        exec.expect(
            "deb-s3",
            |a| a.first() == Some(&"exist") && a.get(1) == Some(&"b-pkg"),
            CommandOutput::success("b-pkg : Missing"),
        );
        let mut args = args_for(root.path());
        args.verify = true;

        let err = execute_with(args, &exec, &S3Config::default())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("b-pkg"), "{}", err);
        assert!(
            !err.to_string().contains("a-pkg"),
            "a package already found must not be reported as missing: {}",
            err
        );
        // a-pkg asked once + b-pkg asked on all 3 attempts.
        assert_eq!(subcommand_calls(&exec, "exist"), 4);
        assert_eq!(subcommand_calls(&exec, "upload"), 1);
    }

    #[tokio::test]
    async fn verify_fails_the_command_when_a_package_never_appears() {
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], absent());
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));
        exec.expect_args_starting_with(
            "deb-s3",
            &["exist"],
            CommandOutput::success("mina-devnet : Missing"),
        );
        let mut args = args_for(root.path());
        args.verify = true;

        let err = execute_with(args, &exec, &S3Config::default())
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("still not listed"),
            "unexpected error: {}",
            err
        );
        // verify_attempts(3) existence checks.
        assert_eq!(subcommand_calls(&exec, "exist"), 3);
    }

    // ---- the pre-flight identity check ----------------------------------

    #[test]
    fn show_argv_asks_about_one_package_positionally() {
        let package = PackageRef {
            name: "mina-devnet".to_string(),
            version: "4.0.0-abc1234".to_string(),
            arch: "amd64".to_string(),
        };

        let argv = show_argv("b", "bullseye", "stable", &package, &S3Config::default());

        assert_eq!(argv[0], "show");
        assert_eq!(argv[1], "mina-devnet");
        assert_eq!(argv[2], "4.0.0-abc1234");
        assert_eq!(argv[3], "amd64");
        // `show` takes the architecture positionally; passing --arch as well
        // would be an unknown option on the fork.
        assert!(!argv.iter().any(|a| a == "--arch"), "argv: {:?}", argv);
        let component = argv.iter().position(|a| a == "--component").unwrap();
        assert_eq!(argv[component + 1], "stable");
    }

    #[test]
    fn sha256_is_read_out_of_a_show_stanza() {
        let stanza =
            published_with("ea1aab812203fe308652663a9026734ef2914413e7e3480720b57742c4eb5b92");

        assert_eq!(
            sha256_from_show_output(&stanza.stdout).as_deref(),
            Some("ea1aab812203fe308652663a9026734ef2914413e7e3480720b57742c4eb5b92")
        );
    }

    #[test]
    fn a_description_that_mentions_sha256_is_not_mistaken_for_the_field() {
        // Continuation lines of a Description are indented by one space. The
        // decoy comes first on purpose: with the real field first, the
        // function would return before ever reaching it and the test would
        // pass with the indentation rule deleted.
        let stanza = "Package: p\n\
                      Description: fixture\n\
                      \x20SHA256: 2222222222222222222222222222222222222222222222222222222222222222\n\
                      SHA256: 1111111111111111111111111111111111111111111111111111111111111111\n";

        assert_eq!(
            sha256_from_show_output(stanza).as_deref(),
            Some("1111111111111111111111111111111111111111111111111111111111111111")
        );
    }

    #[test]
    fn the_pool_key_is_read_out_of_a_show_stanza() {
        let stanza = published_with(&fixture_sha256());

        assert_eq!(
            pool_key_from_show_output(&stanza.stdout).as_deref(),
            Some("pool/bullseye/m/mi/mina-devnet_4.0.0-abc1234_amd64.deb")
        );
        assert_eq!(pool_key_from_show_output("Package: p\nSize: 1\n"), None);

        // Decoy first, as in the SHA256 test: without the indentation rule
        // this would read the description's line.
        let with_decoy = "Package: p\n\
                          Description: fixture\n\
                          \x20Filename: pool/wrong/key.deb\n\
                          Filename: pool/right/key.deb\n";
        assert_eq!(
            pool_key_from_show_output(with_decoy).as_deref(),
            Some("pool/right/key.deb")
        );
    }

    #[test]
    fn the_size_and_md5_are_read_out_of_a_show_stanza() {
        let stanza = published_with(&fixture_sha256());

        assert_eq!(size_from_show_output(&stanza.stdout), Some(STANZA_SIZE));
        assert_eq!(
            md5_from_show_output(&stanza.stdout).as_deref(),
            Some(STANZA_MD5)
        );
        // Present but unusable is not an answer.
        assert_eq!(size_from_show_output("Size: huge\n"), None);
        assert_eq!(md5_from_show_output("MD5sum: nope\n"), None);
    }

    #[test]
    fn head_object_argv_names_the_bucket_the_key_and_any_custom_endpoint() {
        let plain = head_object_argv("b", "pool/x/y.deb", &S3Config::default());
        assert_eq!(&plain[0..2], &["s3api", "head-object"]);
        let key = plain.iter().position(|a| a == "--key").unwrap();
        assert_eq!(plain[key + 1], "pool/x/y.deb");
        assert!(!plain.iter().any(|a| a == "--endpoint-url"));

        let s3 = S3Config {
            endpoint: Some("http://127.0.0.1:9000".to_string()),
            // Set on purpose: the assertion below is only worth anything if
            // there is a credential that *could* have been leaked.
            access_key_id: Some("AKIAEXAMPLE".to_string()),
            secret_access_key: Some("s3cr3t-key-material".to_string()),
            force_path_style: true,
        };
        let against_minio = head_object_argv("b", "pool/x/y.deb", &s3);
        let endpoint = against_minio
            .iter()
            .position(|a| a == "--endpoint-url")
            .unwrap();
        assert_eq!(against_minio[endpoint + 1], "http://127.0.0.1:9000");
        // Credentials must never reach the process list — `aws` reads them
        // from the environment.
        assert!(
            !against_minio
                .iter()
                .any(|a| a.contains("s3cr3t") || a.contains("AKIAEXAMPLE")),
            "argv: {:?}",
            against_minio
        );
    }

    #[test]
    fn a_stanza_without_a_usable_sha256_gives_no_answer_rather_than_a_wrong_one() {
        assert_eq!(sha256_from_show_output("Package: p\nSize: 736\n"), None);
        assert_eq!(sha256_from_show_output("SHA256: not-a-digest\n"), None);
        assert_eq!(sha256_from_show_output(""), None);
    }

    #[tokio::test]
    async fn an_identical_package_already_published_is_skipped_not_re_uploaded() {
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], published_with(&fixture_sha256()));
        exec.expect_args_starting_with("aws", &["s3api", "head-object"], pool_present());
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(
            subcommand_calls(&exec, "upload"),
            0,
            "a package already published with the same bytes must not be re-uploaded"
        );
    }

    #[tokio::test]
    async fn a_package_the_manifest_lists_but_the_pool_is_missing_is_uploaded_again() {
        // deb-s3 writes the manifest before the .deb (cli.rb:254-283), so a
        // run killed while the packages were transferring leaves an index
        // that advertises a package whose pool object does not exist. The
        // retry has to repair that, not skip it — skipping would report
        // success over a repository that 404s on install, permanently.
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], published_with(&fixture_sha256()));
        exec.expect_args_starting_with("aws", &["s3api", "head-object"], pool_missing());
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(
            subcommand_calls(&exec, "upload"),
            1,
            "the manifest alone must not be enough to skip an upload"
        );
    }

    #[tokio::test]
    async fn a_pool_check_that_cannot_run_uploads_rather_than_skipping() {
        // `aws` not on PATH at all: the spawn itself fails, which is not the
        // same code path as a non-zero exit and has to be covered separately.
        // None of these are an answer, and the safe side of an unanswered
        // question is to upload the identical bytes again.
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = UnspawnableAws {
            mock: MockCommandExecutor::new(),
        };
        exec.mock
            .expect_args_starting_with("deb-s3", &["show"], published_with(&fixture_sha256()));
        exec.mock
            .expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(subcommand_calls(&exec.mock, "upload"), 1);
    }

    /// An executor for which `aws` cannot be spawned at all — `exec.run`
    /// returns `Err`, which [`MockCommandExecutor`] can never produce (an
    /// unmatched call is an `Ok` with status 127).
    struct UnspawnableAws {
        mock: MockCommandExecutor,
    }

    impl CommandExecutor for UnspawnableAws {
        fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutput> {
            if program == "aws" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "No such file or directory (os error 2)",
                ));
            }
            self.mock.run(program, args)
        }
    }

    #[tokio::test]
    async fn a_pool_object_holding_the_previous_build_is_uploaded_over() {
        // The manifest says our bytes and the key exists — but it holds
        // something else, which apt reports as a hash mismatch rather than a
        // 404. Existence alone must not be enough to skip.
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], published_with(&fixture_sha256()));
        exec.expect_args_starting_with(
            "aws",
            &["s3api", "head-object"],
            pool_holding(STANZA_SIZE, "ffffffffffffffffffffffffffffffff"),
        );
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(subcommand_calls(&exec, "upload"), 1);
    }

    #[tokio::test]
    async fn a_pool_object_of_the_wrong_size_is_uploaded_over() {
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], published_with(&fixture_sha256()));
        exec.expect_args_starting_with(
            "aws",
            &["s3api", "head-object"],
            pool_holding(STANZA_SIZE + 1, STANZA_MD5),
        );
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(subcommand_calls(&exec, "upload"), 1);
    }

    #[tokio::test]
    async fn a_head_object_answer_with_no_comparable_digest_is_uploaded_over() {
        // A multipart ETag (`<hash>-<parts>`) and no deb-s3 md5 metadata: the
        // object cannot be matched against the index, so it is not a skip.
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], published_with(&fixture_sha256()));
        exec.expect_args_starting_with(
            "aws",
            &["s3api", "head-object"],
            CommandOutput::success(
                "{\n    \"ContentLength\": 736,\n    \"ETag\": \"\\\"abc123-7\\\"\"\n}\n",
            ),
        );
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(subcommand_calls(&exec, "upload"), 1);
    }

    #[tokio::test]
    async fn a_stanza_with_no_filename_is_uploaded_over_rather_than_failing() {
        // The digests already matched, so uploading again is known to be
        // safe. Failing here would leave the operator choosing between a
        // stuck pipeline and a --force that disables the guard for every
        // package in the run.
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with(
            "deb-s3",
            &["show"],
            CommandOutput::success(format!(
                "Package: mina-devnet\nVersion: 4.0.0-abc1234\nSHA256: {}\n",
                fixture_sha256()
            )),
        );
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(subcommand_calls(&exec, "upload"), 1);
        assert_eq!(
            exec.call_count("aws"),
            0,
            "there is no key to ask about, so nothing should be asked"
        );
    }

    #[tokio::test]
    async fn an_arch_all_package_is_never_skipped() {
        // deb-s3 merges an Architecture: all package into every architecture
        // manifest that exists at upload time (cli.rb:229-243). Skipping one
        // leaves an architecture that appeared since it was first published
        // without it.
        let root = make_tree(&[("bullseye", &["mina-devnet-config_4.0.0-abc1234_all.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], published_with(&fixture_sha256()));
        exec.expect_args_starting_with("aws", &["s3api", "head-object"], pool_present());
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(subcommand_calls(&exec, "upload"), 1);
    }

    #[tokio::test]
    async fn a_mismatch_in_a_later_codename_fails_before_the_first_one_is_published() {
        // Mina publishes several codenames in one invocation. A refusal in
        // the last of them must not leave the first ones published.
        let root = make_tree(&[
            ("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"]),
            ("noble", &["mina-devnet_4.0.0-abc1234_amd64.deb"]),
        ]);
        let exec = MockCommandExecutor::new();
        exec.expect(
            "deb-s3",
            |a| a.first() == Some(&"show") && a.contains(&"bullseye"),
            absent(),
        );
        exec.expect(
            "deb-s3",
            |a| a.first() == Some(&"show") && a.contains(&"noble"),
            published_with(MISMATCHED_DIGEST),
        );
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        let err = execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("different contents"), "{}", err);
        assert_eq!(
            subcommand_calls(&exec, "upload"),
            0,
            "bullseye must not be published when noble is going to be refused"
        );
    }

    #[tokio::test]
    async fn a_different_package_at_the_same_version_is_a_hard_error() {
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let repo_digest = MISMATCHED_DIGEST;
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], published_with(repo_digest));
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        let err = execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("different contents"), "{}", err);
        assert!(
            err.contains(repo_digest),
            "repository digest missing: {}",
            err
        );
        assert!(
            err.contains(&fixture_sha256()),
            "local digest missing: {}",
            err
        );
        assert!(err.contains("--force"), "{}", err);
        assert_eq!(
            subcommand_calls(&exec, "upload"),
            0,
            "nothing may be uploaded once a mismatch is found"
        );
    }

    #[tokio::test]
    async fn a_mix_of_new_and_already_published_uploads_only_the_new_ones() {
        let root = make_tree(&[(
            "bullseye",
            &[
                "a-pkg_4.0.0-abc1234_amd64.deb",
                "b-pkg_4.0.0-abc1234_amd64.deb",
            ],
        )]);
        let digest = fixture_sha256();
        let exec = MockCommandExecutor::new();
        exec.expect(
            "deb-s3",
            |a| a.first() == Some(&"show") && a.get(1) == Some(&"a-pkg"),
            published_with(&digest),
        );
        exec.expect(
            "deb-s3",
            |a| a.first() == Some(&"show") && a.get(1) == Some(&"b-pkg"),
            absent(),
        );
        exec.expect_args_starting_with("aws", &["s3api", "head-object"], pool_present());
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap();

        let calls = exec.calls.lock().unwrap();
        let upload = calls
            .iter()
            .find(|c| c.args.first().map(String::as_str) == Some("upload"))
            .expect("the new package must still be uploaded");
        assert!(
            upload
                .args
                .iter()
                .any(|a| a.ends_with("b-pkg_4.0.0-abc1234_amd64.deb")),
            "argv: {:?}",
            upload.args
        );
        assert!(
            !upload
                .args
                .iter()
                .any(|a| a.ends_with("a-pkg_4.0.0-abc1234_amd64.deb")),
            "the already-published package must be left out of the upload: {:?}",
            upload.args
        );
    }

    #[tokio::test]
    async fn verify_still_covers_packages_that_were_skipped() {
        // The whole point of skipping is that the package is already there.
        // --verify has to prove that, not take the skip's word for it.
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["show"], published_with(&fixture_sha256()));
        exec.expect_args_starting_with("aws", &["s3api", "head-object"], pool_present());
        exec.expect_args_starting_with(
            "deb-s3",
            &["exist"],
            CommandOutput::success("mina-devnet : Found"),
        );
        let mut args = args_for(root.path());
        args.verify = true;

        execute_with(args, &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(subcommand_calls(&exec, "upload"), 0);
        assert_eq!(subcommand_calls(&exec, "exist"), 1);
    }

    #[tokio::test]
    async fn an_unreadable_show_output_is_an_error_not_an_assumption() {
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with(
            "deb-s3",
            &["show"],
            CommandOutput::success("Package: mina-devnet\nVersion: 4.0.0-abc1234\n"),
        );
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        let err = execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("no readable SHA256"), "{}", err);
        assert_eq!(subcommand_calls(&exec, "upload"), 0);
    }

    #[tokio::test]
    async fn a_show_that_failed_for_another_reason_is_not_read_as_absent() {
        // Exit 1 with anything but "No such package found." is a failure to
        // read the repository. Treating it as absent would upload over an
        // answer we never got.
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with(
            "deb-s3",
            &["show"],
            CommandOutput::failure(1, "Aws::S3::Errors::AccessDenied"),
        );
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));

        let err = execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("cannot tell what the repository holds"),
            "{}",
            err
        );
        assert!(err.contains("AccessDenied"), "{}", err);
        assert_eq!(subcommand_calls(&exec, "upload"), 0);
    }

    #[tokio::test]
    async fn force_skips_the_preflight_entirely() {
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
        // The same digest `a_different_package_at_the_same_version_is_a_hard_error`
        // uses: without --force this exact setup fails the command.
        exec.expect_args_starting_with("deb-s3", &["show"], published_with(MISMATCHED_DIGEST));
        exec.expect_args_starting_with("deb-s3", &["upload"], CommandOutput::success(""));
        let mut args = args_for(root.path());
        args.force = true;

        execute_with(args, &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(subcommand_calls(&exec, "show"), 0);
        assert_eq!(subcommand_calls(&exec, "upload"), 1);
    }

    #[tokio::test]
    async fn a_name_the_preflight_cannot_read_fails_before_anything_is_uploaded() {
        let root = make_tree(&[("bullseye", &["garbage.deb"])]);
        let exec = ok_exec();

        let err = execute_with(args_for(root.path()), &exec, &S3Config::default())
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("Cannot read name/version/arch"), "{}", err);
        assert_eq!(subcommand_calls(&exec, "upload"), 0);
    }

    /// End-to-end against a real `deb-s3` and a real (MinIO) S3 bucket:
    /// build three tiny `.deb` fixtures, upload them with the command under
    /// test, then ask `deb-s3 list` whether they are actually in the
    /// component. Nothing is mocked except `dig` and `aws cloudfront`, which
    /// this team does not use in production.
    ///
    /// Then the part that pins the semantics of the pre-flight, against the
    /// real deb-s3 rather than a mock of it: publishing the same folder a
    /// second time succeeds and changes nothing, and publishing *different*
    /// bytes at the same version fails and leaves the published package
    /// alone. Without the pre-flight the third run would exit 0 and replace
    /// what the repository serves — `--fail-if-exists` does not stop it.
    ///
    /// Gated behind the `integration-test` feature because it needs Docker,
    /// `deb-s3` and `dpkg-deb` on PATH. Run with:
    /// `cargo test --features integration-test publish_against_minio`.
    #[cfg(feature = "integration-test")]
    #[tokio::test]
    async fn publish_against_minio() {
        use crate::process::{command_available, run_with_env, MixedExecutor};
        use testcontainers_modules::minio::MinIO;
        use testcontainers_modules::testcontainers::runners::AsyncRunner;

        for tool in ["docker", "deb-s3", "dpkg-deb", "aws"] {
            if !command_available(tool) {
                eprintln!("skipping publish_against_minio: {} not on PATH", tool);
                return;
            }
        }

        let container = MinIO::default()
            .start()
            .await
            .expect("minio container start");
        let host_port = container
            .get_host_port_ipv4(9000)
            .await
            .expect("minio port");
        let endpoint = format!("http://127.0.0.1:{}", host_port);
        let access_key = "minioadmin";
        let secret_key = "minioadmin";
        let bucket = "upload-test-bucket";

        let aws_env: Vec<(&str, &str)> = vec![
            ("AWS_ACCESS_KEY_ID", access_key),
            ("AWS_SECRET_ACCESS_KEY", secret_key),
            ("AWS_REGION", "us-east-1"),
            ("AWS_EC2_METADATA_DISABLED", "true"),
        ];
        run_with_env(
            "aws",
            &[
                "--endpoint-url",
                &endpoint,
                "s3",
                "mb",
                &format!("s3://{}", bucket),
            ],
            &aws_env,
        );
        for (k, v) in &aws_env {
            std::env::set_var(k, v);
        }

        // Two amd64 packages in one codename folder: this is what proves the
        // batching is right, because a single deb-s3 call has to carry both.
        // The third is `Architecture: all`, which mina really ships (the
        // network config packages) and which deb-s3 keeps in its own manifest
        // as well as merging into each real architecture's — so it is the
        // case worth pinning, for the pre-flight read and for --verify alike.
        let tmp = tempfile::tempdir().unwrap();
        let codename_dir = tmp.path().join("bullseye");
        std::fs::create_dir_all(&codename_dir).unwrap();

        // Build one fixture `.deb` into the codename folder. `payload` is
        // written inside the package, so the same name+version can be built
        // twice with different bytes.
        let build_deb = |name: &str, arch: &str, payload: &str| {
            let pkg_root = tmp.path().join(format!("{}-root", name));
            let _ = std::fs::remove_dir_all(&pkg_root);
            std::fs::create_dir_all(pkg_root.join("DEBIAN")).unwrap();
            std::fs::create_dir_all(pkg_root.join("usr/share/doc").join(name)).unwrap();
            std::fs::write(
                pkg_root.join("DEBIAN/control"),
                format!(
                    "Package: {}\n\
                     Version: 4.0.0-abc1234\n\
                     Architecture: {}\n\
                     Maintainer: test@example.com\n\
                     Suite: stable\n\
                     Description: upload integration fixture\n",
                    name, arch
                ),
            )
            .unwrap();
            std::fs::write(
                pkg_root.join("usr/share/doc").join(name).join("README"),
                payload,
            )
            .unwrap();
            let deb = codename_dir.join(format!("{}_4.0.0-abc1234_{}.deb", name, arch));
            let out = std::process::Command::new("dpkg-deb")
                .args(["-Zgzip", "--build"])
                .arg(&pkg_root)
                .arg(&deb)
                .output()
                .expect("dpkg-deb");
            assert!(
                out.status.success(),
                "dpkg-deb failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            deb
        };

        build_deb("mina-devnet", "amd64", "original\n");
        build_deb("mina-archive-devnet", "amd64", "original\n");
        build_deb("mina-devnet-config", "all", "original\n");

        // Only `dig` is mocked. `aws` runs for real, because the pre-flight's
        // pool-object check goes through it and mocking that away would make
        // the skip prove nothing. The CloudFront half of `aws` is never
        // reached: `dig` answers with no CNAME, so `invalidate_cloudfront`
        // returns before any `aws cloudfront` call.
        let exec = MixedExecutor::new(&["dig"]);
        exec.mock.expect_args_starting_with(
            "dig",
            &["+short", "CNAME"],
            CommandOutput::success("\n"),
        );

        let s3 = S3Config {
            endpoint: Some(endpoint.clone()),
            access_key_id: Some(access_key.to_string()),
            secret_access_key: Some(secret_key.to_string()),
            force_path_style: true,
        };

        // `PublishArgs` is not `Clone`, and this test publishes three times.
        let args_for_run = || PublishArgs {
            source_folder: tmp.path().to_string_lossy().into_owned(),
            debian_repo: format!("{}/{}", endpoint, bucket),
            channel: "stable".to_string(),
            codenames: Some("bullseye".to_string()),
            debian_sign_key: None,
            force: false,
            verify: true,
            verify_attempts: 5,
            verify_interval_secs: 2,
            skip_cache_invalidation: false,
            dry_run: false,
            s3_endpoint: None,
            s3_force_path_style: false,
        };

        // A `deb-s3` read against the same repository this test publishes to.
        let deb_s3 = |args: &[&str]| -> std::process::Output {
            std::process::Command::new("deb-s3")
                .args(args)
                .args([
                    &format!("--bucket={}", bucket),
                    "--endpoint",
                    &endpoint,
                    "--access-key-id",
                    access_key,
                    "--secret-access-key",
                    secret_key,
                    "--force-path-style",
                    "--codename",
                    "bullseye",
                    "--component",
                    "stable",
                ])
                .output()
                .expect("deb-s3")
        };

        execute_with(args_for_run(), &exec, &s3)
            .await
            .expect("publish against MinIO failed");

        // The claim under test: the packages are readable back out of the
        // stable component at the version they were built with.
        let listed = deb_s3(&["list", "--arch", "amd64"]);
        let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
        assert!(
            listed.contains("mina-devnet") && listed.contains("mina-archive-devnet"),
            "packages missing from the stable component: {}",
            listed
        );
        assert!(
            listed.contains("4.0.0-abc1234"),
            "version was rewritten during upload — it must not be: {}",
            listed
        );

        let original = deb_s3(&["show", "mina-devnet", "4.0.0-abc1234", "amd64"]);
        assert!(original.status.success(), "deb-s3 show failed after upload");
        let original_sha256 = sha256_from_show_output(&String::from_utf8_lossy(&original.stdout))
            .expect("no SHA256 in the show stanza");

        // A `deb-s3` read against the same repository this test publishes to.
        let aws = |args: &[&str]| -> std::process::Output {
            std::process::Command::new("aws")
                .args(["--endpoint-url", &endpoint, "--region", S3_REGION])
                .args(args)
                .output()
                .expect("aws")
        };
        let pool_key = "pool/bullseye/m/mi/mina-devnet_4.0.0-abc1234_amd64.deb";
        let head = |key: &str| -> std::process::Output {
            aws(&["s3api", "head-object", "--bucket", bucket, "--key", key])
        };
        let head_before = head(pool_key);
        assert!(
            head_before.status.success(),
            "the pool object should exist after the first publish: {}",
            String::from_utf8_lossy(&head_before.stderr)
        );
        let modified_before = String::from_utf8_lossy(&head_before.stdout).into_owned();

        // 1. Re-publishing the identical folder converges: this is the retry
        //    after a partial upload, and it must not need a human.
        execute_with(args_for_run(), &exec, &s3)
            .await
            .expect("re-publishing the same packages must succeed");

        // …and the skip is a real skip: an untouched pool object keeps its
        // LastModified. Without this the whole check could be silently
        // degraded — every package re-uploaded — and every other assertion
        // here would still pass.
        assert_eq!(
            String::from_utf8_lossy(&head(pool_key).stdout),
            modified_before,
            "the pool object was rewritten — the package was not skipped"
        );

        let relisted = deb_s3(&["list", "--arch", "amd64"]);
        let relisted = String::from_utf8_lossy(&relisted.stdout).into_owned();
        assert_eq!(
            relisted.matches("mina-devnet ").count(),
            listed.matches("mina-devnet ").count(),
            "a re-publish must not duplicate entries: {}",
            relisted
        );

        // The `all` package is found through its own manifest, the same way
        // --verify finds it: `show` and `exist` share the lookup, so if this
        // holds, an arch-all package is covered by the pre-flight too.
        let config = deb_s3(&["show", "mina-devnet-config", "4.0.0-abc1234", "all"]);
        assert!(
            config.status.success(),
            "an Architecture: all package must be readable back: {}",
            String::from_utf8_lossy(&config.stderr)
        );

        // 2. The case a manifest read alone cannot see. deb-s3 writes the
        //    index before the `.deb` (cli.rb:254-283), so a run killed while
        //    the packages were transferring leaves exactly this state: the
        //    stanza is there, with the right SHA256, and the pool object is
        //    not. The retry must repair it rather than skip it.
        let deleted = aws(&[
            "s3api",
            "delete-object",
            "--bucket",
            bucket,
            "--key",
            pool_key,
        ]);
        assert!(
            deleted.status.success(),
            "could not delete the pool object: {}",
            String::from_utf8_lossy(&deleted.stderr)
        );
        assert!(
            !head(pool_key).status.success(),
            "the pool object should be gone at this point"
        );

        execute_with(args_for_run(), &exec, &s3)
            .await
            .expect("a publish over a missing pool object must succeed");

        assert!(
            head(pool_key).status.success(),
            "the missing .deb was never re-uploaded — the manifest was trusted on its own"
        );

        // 3. A different build at the same version is refused, and the
        //    published package is left exactly as it was.
        build_deb("mina-devnet", "amd64", "TAMPERED — a different build\n");

        let err = execute_with(args_for_run(), &exec, &s3)
            .await
            .expect_err("publishing different bytes at the same version must fail")
            .to_string();
        assert!(err.contains("different contents"), "{}", err);
        assert!(err.contains(&original_sha256), "{}", err);

        let after = deb_s3(&["show", "mina-devnet", "4.0.0-abc1234", "amd64"]);
        let after_sha256 = sha256_from_show_output(&String::from_utf8_lossy(&after.stdout))
            .expect("no SHA256 in the show stanza");
        assert_eq!(
            after_sha256, original_sha256,
            "the published package was replaced — the refusal did not hold"
        );
    }

    #[tokio::test]
    async fn the_cdn_is_invalidated_unless_the_caller_opts_out() {
        let root = make_tree(&[("bullseye", &["a_1_amd64.deb"])]);
        let exec = ok_exec();
        exec.expect("dig", |_| true, CommandOutput::success(""));
        let mut args = args_for(root.path());
        args.skip_cache_invalidation = false;

        execute_with(args, &exec, &S3Config::default())
            .await
            .unwrap();

        assert_eq!(exec.call_count("dig"), 1);
    }
}
