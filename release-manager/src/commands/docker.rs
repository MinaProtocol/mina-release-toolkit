//! `release-manager docker` — copy and verify fully-named image references.
//!
//! The caller names every image completely (`<registry>/<name>:<tag>`). This
//! module does not derive tags from artifact, network, profile or build flag:
//! mina's pipeline already computes those to build the images, and a second
//! copy of the naming rules here would drift from it.

use crate::cli::{DockerPromoteArgs, DockerVerifyArgs};
use crate::errors::{ManagerError, ManagerResult};
use crate::process::{CommandExecutor, RealExecutor};

/// Mount point of `--check-scripts-dir` inside the verified container.
const CHECKS_MOUNT: &str = "/checks";

pub async fn promote(args: DockerPromoteArgs) -> ManagerResult<()> {
    promote_with(&args, &RealExecutor)
}

pub async fn verify(args: DockerVerifyArgs) -> ManagerResult<()> {
    verify_with(&args, &RealExecutor)
}

/// Pull `source` once, then tag and push it as every `target`.
pub fn promote_with(args: &DockerPromoteArgs, exec: &dyn CommandExecutor) -> ManagerResult<()> {
    validate_ref(&args.source)?;
    if args.targets.is_empty() {
        return Err(ManagerError::ValidationError(
            "at least one --target is required".to_string(),
        ));
    }
    for target in &args.targets {
        validate_ref(target)?;
    }
    let platform = platform(&args.arch)?;

    println!(" 🐋 Promoting {}", args.source);
    for target in &args.targets {
        println!("    📤 {}", target);
    }
    if args.dry_run {
        println!("    ℹ️  dry run: nothing pulled or pushed");
        return Ok(());
    }

    run(exec, &["pull", "--platform", &platform, &args.source])?;
    for target in &args.targets {
        run(exec, &["tag", &args.source, target])?;
        run(exec, &["push", target])?;
    }
    Ok(())
}

/// Run the package's check script from `--check-scripts-dir` inside `image`.
///
/// The directory must hold `resolve-check-script.sh`, which maps a package
/// name to the check script to run (mina's `scripts/verify/`).
pub fn verify_with(args: &DockerVerifyArgs, exec: &dyn CommandExecutor) -> ManagerResult<()> {
    validate_ref(&args.image)?;
    if args.package.is_empty() {
        return Err(ManagerError::ValidationError(
            "--package cannot be empty".to_string(),
        ));
    }
    let resolver = args.check_scripts_dir.join("resolve-check-script.sh");
    if !resolver.is_file() {
        return Err(ManagerError::ValidationError(format!(
            "{} not found",
            resolver.display()
        )));
    }
    let checks_dir = std::fs::canonicalize(&args.check_scripts_dir).map_err(|e| {
        ManagerError::ValidationError(format!(
            "cannot resolve {}: {}",
            args.check_scripts_dir.display(),
            e
        ))
    })?;
    let platform = platform(&args.arch)?;

    println!(" 🐋 Verifying {} ({})", args.image, args.package);
    if !args.no_pull {
        run(exec, &["pull", "--platform", &platform, &args.image])?;
    }

    let mount = format!("{}:{}:ro", checks_dir.display(), CHECKS_MOUNT);
    // The resolver sets CHECK_SCRIPT relative to its own location, so it
    // resolves to a path under the mount.
    let script = format!(
        "source {}/resolve-check-script.sh \"$1\" && bash \"$CHECK_SCRIPT\"",
        CHECKS_MOUNT
    );
    run(
        exec,
        &[
            "run",
            "--rm",
            "--platform",
            &platform,
            "--entrypoint",
            "bash",
            "-v",
            &mount,
            &args.image,
            "-c",
            &script,
            "check",
            &args.package,
        ],
    )?;
    println!("    ✅ {} passed its checks", args.image);
    Ok(())
}

fn run(exec: &dyn CommandExecutor, args: &[&str]) -> ManagerResult<()> {
    let out = exec
        .run("docker", args)
        .map_err(|e| ManagerError::CommandFailed(format!("docker {}: {}", args.join(" "), e)))?;
    if !out.is_success() {
        return Err(ManagerError::CommandFailed(format!(
            "docker {} exited {}: {}{}",
            args.join(" "),
            out.status,
            out.stdout,
            out.stderr
        )));
    }
    Ok(())
}

fn platform(arch: &str) -> ManagerResult<String> {
    match arch {
        "amd64" | "arm64" => Ok(format!("linux/{}", arch)),
        other => Err(ManagerError::ValidationError(format!(
            "unknown architecture '{}': expected amd64 or arm64",
            other
        ))),
    }
}

/// A reference must name a repository and a tag: `<repo>/<name>:<tag>`.
/// A bare name would silently resolve against Docker Hub's `library/`.
fn validate_ref(image: &str) -> ManagerResult<()> {
    let (repo, tag) = match image.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => (repo, tag),
        _ => {
            return Err(ManagerError::ValidationError(format!(
                "'{}' has no tag; expected <registry>/<name>:<tag>",
                image
            )))
        }
    };
    if tag.is_empty() || !repo.contains('/') || repo.ends_with('/') || repo.starts_with('/') {
        return Err(ManagerError::ValidationError(format!(
            "'{}' is not <registry>/<name>:<tag>",
            image
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{CommandOutput, MockCommandExecutor};
    use std::path::{Path, PathBuf};

    const SRC: &str = "europe-west3-docker.pkg.dev/o1labs-192920/euro-docker-repo/mina-daemon:4.0.1-bookworm-devnet";
    const DST: &str = "docker.io/minaprotocol/mina-daemon:4.0.1-bookworm-devnet";

    fn promote_args(targets: &[&str]) -> DockerPromoteArgs {
        DockerPromoteArgs {
            source: SRC.to_string(),
            targets: targets.iter().map(|s| s.to_string()).collect(),
            arch: "amd64".to_string(),
            dry_run: false,
        }
    }

    fn checks_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("resolve-check-script.sh"), "").unwrap();
        dir
    }

    fn verify_args(dir: &Path) -> DockerVerifyArgs {
        DockerVerifyArgs {
            image: DST.to_string(),
            package: "mina-daemon".to_string(),
            check_scripts_dir: PathBuf::from(dir),
            arch: "amd64".to_string(),
            no_pull: false,
        }
    }

    fn ok_docker() -> MockCommandExecutor {
        let exec = MockCommandExecutor::new();
        exec.expect("docker", |_| true, CommandOutput::success(""));
        exec
    }

    fn argvs(exec: &MockCommandExecutor) -> Vec<Vec<String>> {
        exec.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.args.clone())
            .collect()
    }

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn promote_pulls_once_then_tags_and_pushes_each_target() {
        let other = "docker.io/minaprotocol/mina-daemon:devnet-latest";
        let exec = ok_docker();
        promote_with(&promote_args(&[DST, other]), &exec).unwrap();
        assert_eq!(
            argvs(&exec),
            vec![
                v(&["pull", "--platform", "linux/amd64", SRC]),
                v(&["tag", SRC, DST]),
                v(&["push", DST]),
                v(&["tag", SRC, other]),
                v(&["push", other]),
            ]
        );
    }

    #[test]
    fn promote_pulls_the_requested_platform() {
        let exec = ok_docker();
        let mut args = promote_args(&[DST]);
        args.arch = "arm64".to_string();
        promote_with(&args, &exec).unwrap();
        assert_eq!(
            argvs(&exec)[0],
            v(&["pull", "--platform", "linux/arm64", SRC])
        );
    }

    #[test]
    fn promote_stops_before_pushing_when_the_pull_fails() {
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("docker", &["pull"], CommandOutput::failure(1, "not found"));
        let err = promote_with(&promote_args(&[DST]), &exec).unwrap_err();
        assert!(err.to_string().contains("not found"), "{}", err);
        assert_eq!(exec.call_count("docker"), 1);
    }

    #[test]
    fn promote_dry_run_runs_nothing() {
        let exec = ok_docker();
        let mut args = promote_args(&[DST]);
        args.dry_run = true;
        promote_with(&args, &exec).unwrap();
        assert_eq!(exec.call_count("docker"), 0);
    }

    #[test]
    fn promote_rejects_bad_input_before_running_anything() {
        let exec = ok_docker();
        for args in [
            promote_args(&[]),
            promote_args(&["mina-daemon:1.0.0"]),
            promote_args(&["docker.io/minaprotocol/mina-daemon"]),
            DockerPromoteArgs {
                arch: "riscv".to_string(),
                ..promote_args(&[DST])
            },
        ] {
            assert!(promote_with(&args, &exec).is_err());
        }
        assert_eq!(exec.call_count("docker"), 0);
    }

    #[test]
    fn validate_ref_accepts_registry_ports_and_rejects_untagged() {
        validate_ref("127.0.0.1:5000/mina-daemon:1.0.0").unwrap();
        validate_ref("docker.io/minaprotocol/mina-daemon:1.0.0-arm64").unwrap();
        assert!(validate_ref("127.0.0.1:5000/mina-daemon").is_err());
        assert!(validate_ref("mina-daemon:1.0.0").is_err());
        assert!(validate_ref("docker.io/minaprotocol/mina-daemon:").is_err());
    }

    #[test]
    fn verify_pulls_then_runs_the_resolved_check_with_the_dir_mounted() {
        let dir = checks_dir();
        let exec = ok_docker();
        verify_with(&verify_args(dir.path()), &exec).unwrap();

        let calls = argvs(&exec);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], v(&["pull", "--platform", "linux/amd64", DST]));
        let run = &calls[1];
        let mount = format!(
            "{}:/checks:ro",
            std::fs::canonicalize(dir.path()).unwrap().display()
        );
        assert_eq!(
            &run[..9],
            &v(&[
                "run",
                "--rm",
                "--platform",
                "linux/amd64",
                "--entrypoint",
                "bash",
                "-v",
                &mount,
                DST
            ])[..]
        );
        assert_eq!(run.last().unwrap(), "mina-daemon");
    }

    #[test]
    fn verify_no_pull_uses_the_local_image() {
        let dir = checks_dir();
        let exec = ok_docker();
        let mut args = verify_args(dir.path());
        args.no_pull = true;
        verify_with(&args, &exec).unwrap();
        let calls = argvs(&exec);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0][0], "run");
    }

    #[test]
    fn verify_fails_when_the_check_fails() {
        let dir = checks_dir();
        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("docker", &["pull"], CommandOutput::success(""));
        exec.expect_args_starting_with("docker", &["run"], CommandOutput::failure(1, "KO"));
        assert!(verify_with(&verify_args(dir.path()), &exec).is_err());
    }

    #[test]
    fn verify_requires_the_resolver_script() {
        let dir = tempfile::tempdir().unwrap();
        let exec = ok_docker();
        let err = verify_with(&verify_args(dir.path()), &exec).unwrap_err();
        assert!(
            err.to_string().contains("resolve-check-script.sh"),
            "{}",
            err
        );
        assert_eq!(exec.call_count("docker"), 0);
    }

    /// End-to-end against two real `registry:2` containers: push a fixture
    /// image to one, promote it into the other under a new tag, delete every
    /// local copy, then verify the promoted reference with a check script.
    /// Nothing is mocked.
    ///
    /// Run with: `cargo test --features integration-test docker_against_registries`.
    #[cfg(feature = "integration-test")]
    #[tokio::test]
    async fn docker_against_registries() {
        use crate::process::{command_available, RealExecutor};
        use testcontainers_modules::testcontainers::core::{IntoContainerPort, WaitFor};
        use testcontainers_modules::testcontainers::runners::AsyncRunner;
        use testcontainers_modules::testcontainers::GenericImage;

        if !command_available("docker") {
            eprintln!("skipping docker_against_registries: docker not on PATH");
            return;
        }

        let registry = || {
            GenericImage::new("registry", "2")
                .with_exposed_port(5000.tcp())
                .with_wait_for(WaitFor::message_on_stderr("listening on"))
        };
        let source_registry = registry().start().await.expect("source registry");
        let target_registry = registry().start().await.expect("target registry");
        // Docker trusts plain-HTTP registries on 127.0.0.0/8 without
        // daemon configuration.
        let source_host = format!(
            "127.0.0.1:{}",
            source_registry.get_host_port_ipv4(5000).await.unwrap()
        );
        let target_host = format!(
            "127.0.0.1:{}",
            target_registry.get_host_port_ipv4(5000).await.unwrap()
        );

        let exec = RealExecutor;
        let docker = |args: &[&str]| {
            let out = exec.run("docker", args).expect("docker runs");
            assert!(out.is_success(), "docker {:?}: {}", args, out.stderr);
        };

        // A fixture image with a marker the check script looks for.
        let ctx = tempfile::tempdir().unwrap();
        std::fs::write(
            ctx.path().join("Dockerfile"),
            "FROM debian:bookworm-slim\nRUN echo promoted > /marker\n",
        )
        .unwrap();
        let source = format!("{}/mina-daemon:1.0.0-bookworm-devnet", source_host);
        let target = format!("{}/mina-daemon:2.0.0-bookworm-devnet", target_host);
        docker(&["build", "-q", "-t", &source, ctx.path().to_str().unwrap()]);
        docker(&["push", &source]);
        docker(&["rmi", &source]);

        promote_with(
            &DockerPromoteArgs {
                source: source.clone(),
                targets: vec![target.clone()],
                arch: "amd64".to_string(),
                dry_run: false,
            },
            &exec,
        )
        .expect("promote");
        // Only the registry copy may satisfy the verify below.
        docker(&["rmi", &source, &target]);

        let checks = tempfile::tempdir().unwrap();
        std::fs::write(
            checks.path().join("resolve-check-script.sh"),
            "VERIFY_DIR=\"$(cd \"$(dirname \"${BASH_SOURCE[0]}\")\" && pwd)\"\n\
             case \"$1\" in mina-*) CHECK_SCRIPT=\"$VERIFY_DIR/check-daemon.sh\" ;; *) exit 1 ;; esac\n",
        )
        .unwrap();
        std::fs::write(
            checks.path().join("check-daemon.sh"),
            "grep -qx promoted /marker\n",
        )
        .unwrap();
        let verify = |package: &str| {
            verify_with(
                &DockerVerifyArgs {
                    image: target.clone(),
                    package: package.to_string(),
                    check_scripts_dir: checks.path().to_path_buf(),
                    arch: "amd64".to_string(),
                    no_pull: false,
                },
                &exec,
            )
        };
        verify("mina-daemon").expect("promoted image passes its check");
        // A package the resolver rejects must fail the verification.
        assert!(verify("not-a-mina-package").is_err());

        let _ = exec.run("docker", &["rmi", &target]);
    }
}
