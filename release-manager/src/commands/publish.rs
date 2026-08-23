//! `release-manager publish` — put already-built `.deb` files into an APT
//! repository, unchanged, then check they are really there.
//!
//! This is the direct-publish path: build, test, publish. The package that
//! reaches the repository is byte for byte the package the build produced.
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

    for batch in &batches {
        println!(" 📦 {} ({} package(s))", batch.codename, batch.debs.len());
        for deb in &batch.debs {
            println!(
                "    • {}",
                deb.file_name().and_then(|s| s.to_str()).unwrap_or("?")
            );
        }

        if args.dry_run {
            println!("    ⏩ Dry run — nothing uploaded");
            println!();
            continue;
        }

        // One deb-s3 call per codename rather than one per package: deb-s3
        // takes the repository lock for the whole invocation, so uploading
        // package by package would take and release it N times and rewrite
        // the manifest N times.
        let argv = upload_argv(
            &bucket,
            &batch.codename,
            &args.channel,
            &batch.debs,
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
        uploaded += batch.debs.len();
        println!("    ✅ Uploaded");

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

    if args.dry_run {
        println!(
            "{}",
            " ✅  Dry run complete — nothing was uploaded.".green()
        );
    } else {
        println!(
            "{}",
            format!(
                " ✅  {} package(s) uploaded to {}/{}.",
                uploaded, args.debian_repo, args.channel
            )
            .green()
        );
    }
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

    fn ok_exec() -> MockCommandExecutor {
        let exec = MockCommandExecutor::new();
        exec.expect("deb-s3", |_| true, CommandOutput::success(""));
        exec
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

        assert_eq!(exec.call_count("deb-s3"), 2);
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
            exec.call_count("deb-s3"),
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
        assert_eq!(quiet.call_count("deb-s3"), 1);

        let checked = MockCommandExecutor::new();
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
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].args[0], "exist");
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
            exec.call_count("deb-s3"),
            2,
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
        // 1 upload + a-pkg asked once + b-pkg asked on all 3 attempts.
        assert_eq!(exec.call_count("deb-s3"), 5);
    }

    #[tokio::test]
    async fn verify_fails_the_command_when_a_package_never_appears() {
        let root = make_tree(&[("bullseye", &["mina-devnet_4.0.0-abc1234_amd64.deb"])]);
        let exec = MockCommandExecutor::new();
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
        // 1 upload + verify_attempts(3) existence checks.
        assert_eq!(exec.call_count("deb-s3"), 4);
    }

    /// End-to-end against a real `deb-s3` and a real (MinIO) S3 bucket:
    /// build two tiny `.deb` fixtures, upload them with the command under
    /// test, then ask `deb-s3 list` whether they are actually in the
    /// component. Nothing is mocked except `dig` and `aws cloudfront`, which
    /// this team does not use in production.
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

        // Two packages in one codename folder: this is what proves the
        // batching is right, because a single deb-s3 call has to carry both.
        let tmp = tempfile::tempdir().unwrap();
        let codename_dir = tmp.path().join("bullseye");
        std::fs::create_dir_all(&codename_dir).unwrap();
        for name in ["mina-devnet", "mina-archive-devnet"] {
            let pkg_root = tmp.path().join(format!("{}-root", name));
            std::fs::create_dir_all(pkg_root.join("DEBIAN")).unwrap();
            std::fs::create_dir_all(pkg_root.join("usr/share/doc").join(name)).unwrap();
            std::fs::write(
                pkg_root.join("DEBIAN/control"),
                format!(
                    "Package: {}\n\
                     Version: 4.0.0-abc1234\n\
                     Architecture: amd64\n\
                     Maintainer: test@example.com\n\
                     Suite: stable\n\
                     Description: upload integration fixture\n",
                    name
                ),
            )
            .unwrap();
            std::fs::write(
                pkg_root.join("usr/share/doc").join(name).join("README"),
                "x\n",
            )
            .unwrap();
            let deb = codename_dir.join(format!("{}_4.0.0-abc1234_amd64.deb", name));
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
        }

        let exec = MixedExecutor::new(&["dig", "aws"]);
        exec.mock.expect_args_starting_with(
            "dig",
            &["+short", "CNAME"],
            CommandOutput::success("\n"),
        );
        exec.mock
            .expect("aws", |_| true, CommandOutput::success(""));

        let s3 = S3Config {
            endpoint: Some(endpoint.clone()),
            access_key_id: Some(access_key.to_string()),
            secret_access_key: Some(secret_key.to_string()),
            force_path_style: true,
        };

        let args = PublishArgs {
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

        execute_with(args, &exec, &s3)
            .await
            .expect("publish against MinIO failed");

        // The claim under test: both packages are readable back out of the
        // stable component at the version they were built with.
        let listed = std::process::Command::new("deb-s3")
            .args([
                "list",
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
                "--arch",
                "amd64",
            ])
            .output()
            .expect("deb-s3 list");
        let listed = String::from_utf8_lossy(&listed.stdout);
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
