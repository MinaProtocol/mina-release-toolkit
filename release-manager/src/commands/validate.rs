use colored::*;
use sha2::{Digest, Sha256};

use crate::artifacts::parse_string_list;
use crate::cli::ValidateArgs;
use crate::errors::{ManagerError, ManagerResult};
use crate::process::{CommandExecutor, RealExecutor};
use crate::utils::print_operation_info;

const S3_REGION: &str = "us-west-2";

pub async fn execute(args: ValidateArgs) -> ManagerResult<()> {
    let exec = RealExecutor;
    let client = reqwest::Client::new();
    execute_with(args, &exec, &client).await
}

/// Same as [`execute`], but with the external-process and HTTP dependencies
/// injected. Tests use this with a `MockCommandExecutor` and a wiremock-
/// backed `reqwest::Client`.
pub async fn execute_with(
    args: ValidateArgs,
    exec: &dyn CommandExecutor,
    http: &reqwest::Client,
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
        println!(
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        );
        println!(
            "  {} / {} / {}",
            args.debian_repo, codename, args.channel
        );
        println!(
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        );
        println!();

        for arch in &archs {
            println!(" 📋 Packages [{}]:", arch);
            let bucket_arg = format!("--bucket={}", args.debian_repo);
            let region_arg = format!("--s3-region={}", S3_REGION);
            let out = exec
                .run(
                    "deb-s3",
                    &[
                        "list",
                        &bucket_arg,
                        &region_arg,
                        "--codename",
                        codename,
                        "--component",
                        &args.channel,
                        "--arch",
                        arch,
                    ],
                )
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
                if !verify_sha256_for_arch(
                    http,
                    &args.debian_repo,
                    codename,
                    &args.channel,
                    arch,
                )
                .await?
                {
                    any_failed = true;
                }
            }

            println!(" 🔍 Verifying manifest structure...");
            let bucket_arg = format!("--bucket={}", args.debian_repo);
            let region_arg = format!("--s3-region={}", S3_REGION);
            let codename_arg = format!("--codename={}", codename);
            let component_arg = format!("--component={}", args.channel);
            let mut verify_args: Vec<&str> = vec![
                "verify",
                &bucket_arg,
                &region_arg,
                &codename_arg,
                &component_arg,
            ];
            if args.fix {
                verify_args.push("--fix-manifests");
                if let Some(key) = args.debian_sign_key.as_deref() {
                    verify_args.push("--sign");
                    verify_args.push(key);
                    println!("    🔧 Fix mode: will repair manifests + re-sign InRelease");
                } else {
                    println!(
                        "    🔧 Fix mode: will repair manifests (unsigned — pass \
                         --debian-sign-key to re-sign InRelease)"
                    );
                }
            }
            let out = exec
                .run("deb-s3", &verify_args)
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
                println!(
                    " 🗑️  Invalidating CloudFront cache for {}...",
                    codename
                );
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
/// pointing at a wiremock or MinIO at `http://127.0.0.1:PORT`), use it as-is;
/// otherwise prepend `https://` to match production.
fn repo_base(debian_repo: &str) -> String {
    if debian_repo.contains("://") {
        debian_repo.trim_end_matches('/').to_string()
    } else {
        format!("https://{}", debian_repo)
    }
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
        let result = execute_with(args, &exec, &http).await;
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
        exec.expect_args_starting_with(
            "deb-s3",
            &["verify"],
            CommandOutput::success("OK"),
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
        let result = execute_with(args, &exec, &http).await;
        assert!(result.is_err(), "expected validate to fail on hash mismatch");
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
        exec.expect_args_starting_with(
            "deb-s3",
            &["list"],
            CommandOutput::success(""),
        );
        exec.expect("deb-s3", |args| args.contains(&"--fix-manifests"), CommandOutput::success("manifests fixed"));
        exec.expect("dig", |args| args.contains(&"CNAME"), CommandOutput::success("d111.cloudfront.net.\n"));
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
        let result = execute_with(args, &exec, &http).await;
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
