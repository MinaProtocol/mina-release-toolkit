use colored::*;
use sha2::{Digest, Sha256};

use crate::artifacts::parse_string_list;
use crate::cli::ValidateArgs;
use crate::errors::{ManagerError, ManagerResult};
use crate::process::{CommandExecutor, RealExecutor, S3Config};
use crate::utils::print_operation_info;

const S3_REGION: &str = "us-west-2";

pub async fn execute(args: ValidateArgs) -> ManagerResult<()> {
    let exec = RealExecutor;
    let client = reqwest::Client::new();
    execute_with(args, &exec, &client, &S3Config::default()).await
}

/// Same as [`execute`], but with the external-process / HTTP / S3 endpoint
/// dependencies injected. Tests use this with a `MockCommandExecutor` (or a
/// `MixedExecutor` against MinIO), a wiremock-backed `reqwest::Client`, and
/// an `S3Config` whose `endpoint` points at MinIO. Production callers pass
/// `S3Config::default()`.
pub async fn execute_with(
    args: ValidateArgs,
    exec: &dyn CommandExecutor,
    http: &reqwest::Client,
    s3: &S3Config,
) -> ManagerResult<()> {
    let codenames = parse_string_list(&args.codenames);
    let archs = parse_string_list(&args.archs);

    print_operation_info(
        "Validating debian repository",
        &[
            ("Repository", args.debian_repo.as_str()),
            ("Channel", args.channel.as_str()),
            ("Codenames", args.codenames.as_str()),
            ("Architectures", args.archs.as_str()),
            ("Fix mode", if args.fix { "yes" } else { "no" }),
        ],
    );

    let mut any_failed = false;

    for codename in &codenames {
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("  {} / {} / {}", args.debian_repo, codename, args.channel);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();

        for arch in &archs {
            println!(" 📋 Packages [{}]:", arch);
            // The bucket flag for `deb-s3 list` is the S3 bucket name —
            // production passes the host name (e.g. `packages.o1test.net`),
            // which doubles as the bucket. In tests with MinIO the
            // `debian_repo` includes the endpoint URL plus the bucket
            // (e.g. `http://127.0.0.1:9000/test-bucket`); the bucket name
            // is the path suffix and the endpoint is supplied separately
            // via `s3` config.
            let bucket = bucket_name(&args.debian_repo);
            let mut argv: Vec<String> = vec![
                "list".to_string(),
                format!("--bucket={}", bucket),
                format!("--s3-region={}", S3_REGION),
                "--codename".to_string(),
                codename.to_string(),
                "--component".to_string(),
                args.channel.clone(),
                "--arch".to_string(),
                arch.to_string(),
            ];
            s3.append_args(&mut argv);
            let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = exec
                .run("deb-s3", &argv_refs)
                .map_err(|e| ManagerError::ValidationError(format!("deb-s3 list: {}", e)))?;
            for line in out.stdout.lines() {
                println!("    {}", line);
            }
            for line in out.stderr.lines() {
                eprintln!("    {}", line);
            }
            println!();
        }

        if !args.list_only {
            for arch in &archs {
                if !verify_sha256_for_arch(http, &args.debian_repo, codename, &args.channel, arch)
                    .await?
                {
                    any_failed = true;
                }
            }

            println!(" 🔍 Verifying manifest structure...");
            let bucket = bucket_name(&args.debian_repo);
            let mut verify_args: Vec<String> = vec![
                "verify".to_string(),
                format!("--bucket={}", bucket),
                format!("--s3-region={}", S3_REGION),
                format!("--codename={}", codename),
                format!("--component={}", args.channel),
            ];
            if args.fix {
                verify_args.push("--fix-manifests".to_string());
                if let Some(key) = args.debian_sign_key.as_deref() {
                    verify_args.push("--sign".to_string());
                    verify_args.push(key.to_string());
                    println!("    🔧 Fix mode: will repair manifests + re-sign InRelease");
                } else {
                    println!(
                        "    🔧 Fix mode: will repair manifests (unsigned — pass \
                         --debian-sign-key to re-sign InRelease)"
                    );
                }
            }
            s3.append_args(&mut verify_args);
            let argv_refs: Vec<&str> = verify_args.iter().map(String::as_str).collect();
            let out = exec
                .run("deb-s3", &argv_refs)
                .map_err(|e| ManagerError::ValidationError(format!("deb-s3 verify: {}", e)))?;
            for line in out.stdout.lines() {
                println!("    {}", line);
            }
            for line in out.stderr.lines() {
                eprintln!("    {}", line);
            }
            if !out.is_success() {
                any_failed = true;
            }

            if args.fix {
                println!();
                println!(" 🗑️  Invalidating CloudFront cache for {}...", codename);
                invalidate_cloudfront(exec, &args.debian_repo, codename)?;
            }
        }

        println!();
    }

    if any_failed {
        println!("{}", " ❌  Some validations failed.".red());
        if !args.fix {
            println!("    Run with --fix to attempt repair.");
        }
        Err(ManagerError::ValidationError(
            "validate found mismatches".into(),
        ))
    } else {
        println!("{}", " ✅  All validations passed.".green());
        Ok(())
    }
}

async fn verify_sha256_for_arch(
    http: &reqwest::Client,
    debian_repo: &str,
    codename: &str,
    channel: &str,
    arch: &str,
) -> ManagerResult<bool> {
    println!(" 🔒 Verifying SHA256 hashes [{}]...", arch);

    let packages_url = format!(
        "{}/dists/{}/{}/binary-{}/Packages",
        repo_base(debian_repo),
        codename,
        channel,
        arch
    );

    let body = match http.get(&packages_url).send().await {
        Ok(resp) if resp.status().is_success() => resp.text().await.map_err(|e| {
            ManagerError::ValidationError(format!("Failed to read Packages body: {}", e))
        })?,
        _ => {
            println!("    ⚠️  Could not fetch {}", packages_url);
            return Ok(false);
        }
    };

    let entries = parse_packages_file(&body);
    let mut mismatches = 0usize;
    let mut total = 0usize;

    for entry in &entries {
        let Some(filename) = entry.filename.as_deref() else {
            continue;
        };
        let Some(expected) = entry.sha256.as_deref() else {
            continue;
        };
        total += 1;

        let pkg_url = format!("{}/{}", repo_base(debian_repo), filename);
        let bytes = match http.get(&pkg_url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    println!(
                        "    ✗ {} {}: failed to read body ({})",
                        entry.package.as_deref().unwrap_or("?"),
                        entry.version.as_deref().unwrap_or("?"),
                        e
                    );
                    mismatches += 1;
                    continue;
                }
            },
            Ok(resp) => {
                println!(
                    "    ✗ {} {}: HTTP {}",
                    entry.package.as_deref().unwrap_or("?"),
                    entry.version.as_deref().unwrap_or("?"),
                    resp.status()
                );
                mismatches += 1;
                continue;
            }
            Err(e) => {
                println!(
                    "    ✗ {} {}: fetch failed ({})",
                    entry.package.as_deref().unwrap_or("?"),
                    entry.version.as_deref().unwrap_or("?"),
                    e
                );
                mismatches += 1;
                continue;
            }
        };

        let actual = format!("{:x}", Sha256::digest(&bytes));
        let pkg = entry.package.as_deref().unwrap_or("?");
        let ver = entry.version.as_deref().unwrap_or("?");
        if actual != expected {
            println!("    ✗ {} {}: SHA256 mismatch", pkg, ver);
            println!("      manifest: {}", expected);
            println!("      actual:   {}", actual);
            mismatches += 1;
        } else {
            println!("    ✓ {} {} OK", pkg, ver);
        }
    }

    if mismatches > 0 {
        println!("    ❌ {} hash mismatch(es) found", mismatches);
    } else if total == 0 {
        println!("    ℹ️  No packages with SHA256 in manifest");
    } else {
        println!("    ✅ All {} hashes valid", total);
    }
    println!();

    Ok(mismatches == 0)
}

#[derive(Debug, Default)]
struct PackagesEntry {
    package: Option<String>,
    version: Option<String>,
    filename: Option<String>,
    sha256: Option<String>,
}

/// Treat `debian_repo` as a URL base. If it already has a scheme (test setups
/// pointing at a wiremock or MinIO at `http://127.0.0.1:PORT/bucket`), use
/// it as-is; otherwise prepend `https://` to match production.
fn repo_base(debian_repo: &str) -> String {
    if debian_repo.contains("://") {
        debian_repo.trim_end_matches('/').to_string()
    } else {
        format!("https://{}", debian_repo)
    }
}

/// Derive the S3 bucket name from a `debian_repo`. In production the host
/// name is the bucket (`packages.o1test.net` → bucket `packages.o1test.net`).
/// In tests the value is a full URL like `http://127.0.0.1:9000/test-bucket`,
/// in which case the bucket is the path segment after the host.
fn bucket_name(debian_repo: &str) -> String {
    if let Some(after_scheme) = debian_repo.split_once("://") {
        let rest = after_scheme.1.trim_end_matches('/');
        if let Some((_, path)) = rest.split_once('/') {
            return path.split('/').next().unwrap_or("").to_string();
        }
        return rest.to_string();
    }
    debian_repo.to_string()
}

fn parse_packages_file(body: &str) -> Vec<PackagesEntry> {
    let mut entries = Vec::new();
    let mut current = PackagesEntry::default();
    for line in body.lines() {
        if line.is_empty() {
            if current.filename.is_some() || current.sha256.is_some() {
                entries.push(std::mem::take(&mut current));
            } else {
                current = PackagesEntry::default();
            }
            continue;
        }
        if let Some((field, value)) = line.split_once(':') {
            let value = value.trim();
            match field.trim() {
                "Package" => current.package = Some(value.to_string()),
                "Version" => current.version = Some(value.to_string()),
                "Filename" => current.filename = Some(value.to_string()),
                "SHA256" => current.sha256 = Some(value.to_string()),
                _ => {}
            }
        }
    }
    if current.filename.is_some() || current.sha256.is_some() {
        entries.push(current);
    }
    entries
}

fn invalidate_cloudfront(
    exec: &dyn CommandExecutor,
    debian_repo: &str,
    codename: &str,
) -> ManagerResult<()> {
    let dig_out = exec.run("dig", &["+short", "CNAME", debian_repo]);
    let cf_domain = match dig_out {
        Ok(out) => out.stdout.trim().trim_end_matches('.').to_string(),
        Err(_) => String::new(),
    };
    if cf_domain.is_empty() {
        println!(
            "    ⚠️  No CNAME found for {} — skipping CDN invalidation",
            debian_repo
        );
        return Ok(());
    }

    let query = format!("DistributionList.Items[?DomainName=='{}'].Id", cf_domain);
    let list_out = exec.run(
        "aws",
        &[
            "cloudfront",
            "list-distributions",
            "--query",
            &query,
            "--output",
            "text",
        ],
    );
    let dist_id = match list_out {
        Ok(out) => out.stdout.trim().to_string(),
        Err(_) => String::new(),
    };
    if dist_id.is_empty() || dist_id == "None" {
        println!("    ⚠️  Could not find CloudFront distribution");
        return Ok(());
    }

    let paths = format!("/dists/{}/*", codename);
    let out = exec
        .run(
            "aws",
            &[
                "cloudfront",
                "create-invalidation",
                "--distribution-id",
                &dist_id,
                "--paths",
                &paths,
            ],
        )
        .map_err(|e| {
            ManagerError::ValidationError(format!("aws cloudfront create-invalidation: {}", e))
        })?;

    for line in out.stdout.lines() {
        println!("    {}", line);
    }
    for line in out.stderr.lines() {
        eprintln!("    {}", line);
    }
    if out.is_success() {
        println!("    ✅ Cache invalidation submitted");
    } else {
        println!("    ⚠️  Cache invalidation command failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_packages_basic() {
        let body = "\
Package: mina-daemon
Version: 1.0.0-bullseye-devnet
Architecture: amd64
Filename: pool/main/m/mina-daemon/mina-daemon_1.0.0-bullseye-devnet_amd64.deb
Size: 12345
SHA256: abc123def456

Package: mina-archive
Version: 1.0.0-bullseye-devnet
Filename: pool/main/m/mina-archive/mina-archive_1.0.0-bullseye-devnet_amd64.deb
SHA256: deadbeef
";
        let entries = parse_packages_file(body);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].package.as_deref(), Some("mina-daemon"));
        assert_eq!(entries[0].sha256.as_deref(), Some("abc123def456"));
        assert_eq!(entries[1].package.as_deref(), Some("mina-archive"));
        assert_eq!(entries[1].sha256.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn parse_packages_handles_trailing_entry_without_blank() {
        let body = "Package: foo\nFilename: foo.deb\nSHA256: aa";
        let entries = parse_packages_file(body);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].filename.as_deref(), Some("foo.deb"));
    }

    #[test]
    fn repo_base_keeps_scheme_when_present() {
        assert_eq!(repo_base("http://localhost:1234"), "http://localhost:1234");
        assert_eq!(repo_base("http://localhost:1234/"), "http://localhost:1234");
        assert_eq!(
            repo_base("https://stable.apt.packages.minaprotocol.com"),
            "https://stable.apt.packages.minaprotocol.com"
        );
    }

    #[test]
    fn repo_base_adds_https_when_scheme_missing() {
        assert_eq!(
            repo_base("packages.o1test.net"),
            "https://packages.o1test.net"
        );
    }

    /// End-to-end-ish test of `validate --list-only`: wiremock serves a fake
    /// Packages file with one .deb whose SHA256 we compute on the fly, plus
    /// the .deb bytes themselves; MockCommandExecutor returns canned
    /// deb-s3 output. No fakeroot, no AWS, no real S3.
    #[tokio::test]
    async fn validate_against_mocked_repo_passes() {
        use crate::process::{CommandOutput, MockCommandExecutor};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // The fake .deb is just a few bytes; we hash it to put the right
        // SHA256 into the Packages manifest.
        let deb_bytes: Vec<u8> = b"!<arch>\nfake .deb bytes\n".to_vec();
        let expected_sha256 = format!("{:x}", Sha256::digest(&deb_bytes));
        let filename = "pool/main/m/mina-daemon/mina-daemon_1.0.0_amd64.deb";

        let packages_body = format!(
            "Package: mina-daemon\n\
             Version: 1.0.0\n\
             Architecture: amd64\n\
             Filename: {}\n\
             Size: {}\n\
             SHA256: {}\n",
            filename,
            deb_bytes.len(),
            expected_sha256
        );

        Mock::given(method("GET"))
            .and(path("/dists/bullseye/develop/binary-amd64/Packages"))
            .respond_with(ResponseTemplate::new(200).set_body_string(packages_body))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/{}", filename)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(deb_bytes))
            .mount(&server)
            .await;

        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with(
            "deb-s3",
            &["list"],
            CommandOutput::success("mina-daemon 1.0.0 amd64"),
        );
        exec.expect_args_starting_with(
            "deb-s3",
            &["verify"],
            CommandOutput::success("Verifying ... OK"),
        );

        let args = ValidateArgs {
            codenames: "bullseye".to_string(),
            channel: "develop".to_string(),
            archs: "amd64".to_string(),
            debian_repo: server.uri(),
            debian_sign_key: None,
            fix: false,
            list_only: false,
        };

        let http = reqwest::Client::new();
        let result = execute_with(args, &exec, &http, &S3Config::default()).await;
        assert!(result.is_ok(), "validate failed: {:?}", result.err());

        // Sanity: we hit deb-s3 twice (list + verify), nothing else
        assert_eq!(exec.call_count("deb-s3"), 2);
        assert_eq!(exec.call_count("aws"), 0); // no --fix
        assert_eq!(exec.call_count("dig"), 0);
    }

    #[tokio::test]
    async fn validate_detects_sha256_mismatch() {
        use crate::process::{CommandOutput, MockCommandExecutor};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let deb_bytes: Vec<u8> = b"actual .deb content".to_vec();
        // Wrong SHA in the manifest — validate should catch it.
        let wrong_sha = "deadbeef".repeat(8);
        let filename = "pool/main/m/foo/foo_1.0.0_amd64.deb";
        let packages_body = format!(
            "Package: foo\nVersion: 1.0.0\nArchitecture: amd64\nFilename: {}\nSize: {}\nSHA256: {}\n",
            filename,
            deb_bytes.len(),
            wrong_sha
        );

        Mock::given(method("GET"))
            .and(path("/dists/bullseye/develop/binary-amd64/Packages"))
            .respond_with(ResponseTemplate::new(200).set_body_string(packages_body))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/{}", filename)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(deb_bytes))
            .mount(&server)
            .await;

        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with(
            "deb-s3",
            &["list"],
            CommandOutput::success("foo 1.0.0 amd64"),
        );
        exec.expect_args_starting_with("deb-s3", &["verify"], CommandOutput::success("OK"));

        let args = ValidateArgs {
            codenames: "bullseye".to_string(),
            channel: "develop".to_string(),
            archs: "amd64".to_string(),
            debian_repo: server.uri(),
            debian_sign_key: None,
            fix: false,
            list_only: false,
        };

        let http = reqwest::Client::new();
        let result = execute_with(args, &exec, &http, &S3Config::default()).await;
        assert!(
            result.is_err(),
            "expected validate to fail on hash mismatch"
        );
    }

    /// Real-binary integration test: spins up MinIO via testcontainers,
    /// uploads a real .deb with real `deb-s3`, then runs validate against
    /// it (mocking only `dig` and `aws cloudfront` since this team uses
    /// Hetzner / Cloudflare, not CloudFront).
    ///
    /// Gated behind the `integration-test` feature because it needs Docker
    /// + `deb-s3` + `dpkg-deb` + `aws` on PATH. Run with:
    /// `cargo test --features integration-test validate_against_minio`.
    #[cfg(feature = "integration-test")]
    #[tokio::test]
    async fn validate_against_minio() {
        use crate::process::{CommandOutput, MixedExecutor};
        use testcontainers_modules::minio::MinIO;
        use testcontainers_modules::testcontainers::runners::AsyncRunner;

        // Skip if the host tools aren't available.
        for tool in ["docker", "deb-s3", "dpkg-deb"] {
            if !command_available(tool) {
                eprintln!("skipping validate_against_minio: {} not on PATH", tool);
                return;
            }
        }

        // ---- 1. start MinIO ----
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
        let bucket = "test-bucket";

        // ---- 2. create bucket + make it world-readable ----
        // Use the AWS SDK config via env so the `aws` CLI talks to MinIO.
        let aws_env: Vec<(&str, &str)> = vec![
            ("AWS_ACCESS_KEY_ID", access_key),
            ("AWS_SECRET_ACCESS_KEY", secret_key),
            ("AWS_REGION", "us-east-1"),
            ("AWS_EC2_METADATA_DISABLED", "true"),
        ];

        if command_available("aws") {
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
            // Public read on /dists/* and /pool/* so validate's bare-HTTP
            // GETs (no AWS signing) can fetch the Packages file and the .deb.
            let policy = serde_json::json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": ["s3:GetObject"],
                    "Resource": [
                        format!("arn:aws:s3:::{}/dists/*", bucket),
                        format!("arn:aws:s3:::{}/pool/*", bucket),
                    ]
                }]
            })
            .to_string();
            let policy_file = std::env::temp_dir().join("minio-policy.json");
            std::fs::write(&policy_file, &policy).unwrap();
            run_with_env(
                "aws",
                &[
                    "--endpoint-url",
                    &endpoint,
                    "s3api",
                    "put-bucket-policy",
                    "--bucket",
                    bucket,
                    "--policy",
                    &format!("file://{}", policy_file.display()),
                ],
                &aws_env,
            );
        } else {
            eprintln!("skipping: aws CLI required to provision bucket policy");
            return;
        }

        // ---- 3. build a tiny .deb fixture ----
        let tmp = tempfile::tempdir().unwrap();
        let pkg_root = tmp.path().join("integration-pkg");
        std::fs::create_dir_all(pkg_root.join("DEBIAN")).unwrap();
        std::fs::create_dir_all(pkg_root.join("usr/share/doc/integration-pkg")).unwrap();
        std::fs::write(
            pkg_root.join("DEBIAN/control"),
            "Package: integration-pkg\n\
             Version: 1.0.0\n\
             Architecture: amd64\n\
             Maintainer: test@example.com\n\
             Description: integration test fixture\n",
        )
        .unwrap();
        std::fs::write(
            pkg_root.join("usr/share/doc/integration-pkg/README"),
            "hello\n",
        )
        .unwrap();
        let deb_path = tmp.path().join("integration-pkg_1.0.0_amd64.deb");
        let out = std::process::Command::new("dpkg-deb")
            .args(["-Zgzip", "--build"])
            .arg(&pkg_root)
            .arg(&deb_path)
            .output()
            .expect("dpkg-deb");
        assert!(
            out.status.success(),
            "dpkg-deb failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // ---- 4. real deb-s3 upload to MinIO ----
        let out = std::process::Command::new("deb-s3")
            .args([
                "upload",
                "--bucket",
                bucket,
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
                "develop",
                "--arch",
                "amd64",
                "--preserve-versions",
                "--visibility",
                "public",
            ])
            .arg(&deb_path)
            .output()
            .expect("deb-s3 upload");
        assert!(
            out.status.success(),
            "deb-s3 upload failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        // ---- 5. run validate against MinIO ----
        // deb-s3 invocations in our code don't set the endpoint/keys
        // automatically. Set them in the *process* environment so the
        // subprocess inherits them, then route deb-s3 through the real
        // executor (Mixed mocks only dig + aws).
        for (k, v) in &aws_env {
            std::env::set_var(k, v);
        }
        // Wrap deb-s3 calls with a shim that injects the MinIO endpoint+keys.
        // Simpler approach: prepend the args via an env-aware test PATH.
        // For this test we instead bypass that by patching our code path to
        // hit MinIO directly through env vars deb-s3 already understands —
        // it accepts --endpoint via env: AWS_S3_ENDPOINT.
        std::env::set_var("AWS_S3_ENDPOINT", &endpoint);

        let exec = MixedExecutor::new(&["dig", "aws"]);
        // Mock the cloud-stuff; we use Hetzner/Cloudflare, not CloudFront.
        exec.mock.expect_args_starting_with(
            "dig",
            &["+short", "CNAME"],
            CommandOutput::success("\n"), // empty CNAME → skip path
        );
        exec.mock
            .expect("aws", |_| true, CommandOutput::success(""));

        let args = ValidateArgs {
            codenames: "bullseye".to_string(),
            channel: "develop".to_string(),
            archs: "amd64".to_string(),
            debian_repo: format!("{}/{}", endpoint, bucket),
            debian_sign_key: None,
            fix: false,
            list_only: false,
        };

        // Real deb-s3 calls go through MixedExecutor → RealExecutor; the
        // S3Config injects --endpoint/keys/--force-path-style so each
        // invocation points at MinIO. dig + aws stay mocked (we use
        // Hetzner/Cloudflare in production, not CloudFront).
        let s3 = S3Config {
            endpoint: Some(endpoint.clone()),
            access_key_id: Some(access_key.to_string()),
            secret_access_key: Some(secret_key.to_string()),
            force_path_style: true,
        };

        let http = reqwest::Client::new();
        let result = execute_with(args, &exec, &http, &s3).await;
        assert!(
            result.is_ok(),
            "validate against MinIO failed: {:?}",
            result.err()
        );
    }

    fn command_available(cmd: &str) -> bool {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {} >/dev/null 2>&1", cmd))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn run_with_env(program: &str, args: &[&str], env: &[(&str, &str)]) {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd
            .output()
            .unwrap_or_else(|e| panic!("spawn {} failed: {}", program, e));
        if !out.status.success() {
            eprintln!(
                "[{} {:?}] exited {}: {} / {}",
                program,
                args,
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[tokio::test]
    async fn validate_fix_mode_calls_dig_and_aws() {
        use crate::process::{CommandOutput, MockCommandExecutor};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // List-only equivalent: just an empty Packages file so no hash
        // checks happen; we're really exercising the --fix path.
        Mock::given(method("GET"))
            .and(path("/dists/bullseye/develop/binary-amd64/Packages"))
            .respond_with(ResponseTemplate::new(200).set_body_string(""))
            .mount(&server)
            .await;

        let exec = MockCommandExecutor::new();
        exec.expect_args_starting_with("deb-s3", &["list"], CommandOutput::success(""));
        exec.expect(
            "deb-s3",
            |args| args.contains(&"--fix-manifests"),
            CommandOutput::success("manifests fixed"),
        );
        exec.expect(
            "dig",
            |args| args.contains(&"CNAME"),
            CommandOutput::success("d111.cloudfront.net.\n"),
        );
        exec.expect(
            "aws",
            |args| args.contains(&"list-distributions"),
            CommandOutput::success("EABCDEFGH123"),
        );
        exec.expect(
            "aws",
            |args| args.contains(&"create-invalidation"),
            CommandOutput::success("{\"Invalidation\":{\"Id\":\"I123\"}}"),
        );

        let args = ValidateArgs {
            codenames: "bullseye".to_string(),
            channel: "develop".to_string(),
            archs: "amd64".to_string(),
            debian_repo: server.uri(),
            debian_sign_key: Some("KEYID".to_string()),
            fix: true,
            list_only: false,
        };

        let http = reqwest::Client::new();
        let result = execute_with(args, &exec, &http, &S3Config::default()).await;
        assert!(result.is_ok(), "validate --fix failed: {:?}", result.err());

        // Verify the right things were invoked
        assert_eq!(exec.call_count("dig"), 1);
        assert_eq!(exec.call_count("aws"), 2); // list-distributions + create-invalidation

        // The --fix-manifests deb-s3 call should have included --sign KEYID
        let calls = exec.calls.lock().unwrap();
        let fix_call = calls
            .iter()
            .find(|c| c.program == "deb-s3" && c.args.contains(&"--fix-manifests".to_string()))
            .expect("--fix-manifests call missing");
        assert!(
            fix_call.args.iter().any(|a| a == "--sign"),
            "expected --sign in fix call: {:?}",
            fix_call.args
        );
        assert!(
            fix_call.args.iter().any(|a| a == "KEYID"),
            "expected KEYID in fix call: {:?}",
            fix_call.args
        );
    }
}
