use colored::*;
use sha2::{Digest, Sha256};
use std::process::Command;

use crate::artifacts::parse_string_list;
use crate::cli::ValidateArgs;
use crate::errors::{ManagerError, ManagerResult};
use crate::utils::print_operation_info;

const S3_REGION: &str = "us-west-2";

pub async fn execute(args: ValidateArgs) -> ManagerResult<()> {
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

        // List packages
        for arch in &archs {
            println!(" 📋 Packages [{}]:", arch);
            let output = Command::new("deb-s3")
                .args([
                    "list",
                    &format!("--bucket={}", args.debian_repo),
                    &format!("--s3-region={}", S3_REGION),
                    "--codename",
                    codename,
                    "--component",
                    &args.channel,
                    "--arch",
                    arch,
                ])
                .output()
                .map_err(|e| ManagerError::ValidationError(format!("deb-s3 list failed: {}", e)))?;
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                println!("    {}", line);
            }
            for line in String::from_utf8_lossy(&output.stderr).lines() {
                eprintln!("    {}", line);
            }
            println!();
        }

        if !args.list_only {
            // SHA256 verification
            for arch in &archs {
                if !verify_sha256_for_arch(
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

            // Structural manifest verification
            println!(" 🔍 Verifying manifest structure...");
            let mut verify_args: Vec<String> = vec![
                "verify".to_string(),
                format!("--bucket={}", args.debian_repo),
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
            let output = Command::new("deb-s3")
                .args(&verify_args)
                .output()
                .map_err(|e| {
                    ManagerError::ValidationError(format!("deb-s3 verify failed: {}", e))
                })?;
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                println!("    {}", line);
            }
            for line in String::from_utf8_lossy(&output.stderr).lines() {
                eprintln!("    {}", line);
            }
            if !output.status.success() {
                any_failed = true;
            }

            // CloudFront invalidation when fixing
            if args.fix {
                println!();
                println!(
                    " 🗑️  Invalidating CloudFront cache for {}...",
                    codename
                );
                invalidate_cloudfront(&args.debian_repo, codename)?;
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

/// Fetch the Packages file for one (codename, channel, arch) and verify each
/// listed .deb's SHA256 against its actual content over HTTPS. Returns true
/// when every package matched.
async fn verify_sha256_for_arch(
    debian_repo: &str,
    codename: &str,
    channel: &str,
    arch: &str,
) -> ManagerResult<bool> {
    println!(" 🔒 Verifying SHA256 hashes [{}]...", arch);

    let packages_url = format!(
        "https://{}/dists/{}/{}/binary-{}/Packages",
        debian_repo, codename, channel, arch
    );

    let body = match reqwest::get(&packages_url).await {
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

    let client = reqwest::Client::new();

    for entry in &entries {
        let Some(filename) = entry.filename.as_deref() else {
            continue;
        };
        let Some(expected) = entry.sha256.as_deref() else {
            continue;
        };
        total += 1;

        let pkg_url = format!("https://{}/{}", debian_repo, filename);
        let bytes = match client.get(&pkg_url).send().await {
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
        // Match `Field: value`
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

/// Mirror bash: dig the CNAME, find the matching CloudFront distribution by
/// domain name, submit an invalidation for `/dists/<codename>/*`.
fn invalidate_cloudfront(debian_repo: &str, codename: &str) -> ManagerResult<()> {
    let dig_out = Command::new("dig")
        .args(["+short", "CNAME", debian_repo])
        .output();
    let cf_domain = match dig_out {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            s.trim_end_matches('.').to_string()
        }
        Err(_) => String::new(),
    };

    if cf_domain.is_empty() {
        println!(
            "    ⚠️  No CNAME found for {} — skipping CDN invalidation",
            debian_repo
        );
        return Ok(());
    }

    let dist = Command::new("aws")
        .args([
            "cloudfront",
            "list-distributions",
            "--query",
            &format!(
                "DistributionList.Items[?DomainName=='{}'].Id",
                cf_domain
            ),
            "--output",
            "text",
        ])
        .output();
    let dist_id = match dist {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => String::new(),
    };

    if dist_id.is_empty() || dist_id == "None" {
        println!("    ⚠️  Could not find CloudFront distribution");
        return Ok(());
    }

    let out = Command::new("aws")
        .args([
            "cloudfront",
            "create-invalidation",
            "--distribution-id",
            &dist_id,
            "--paths",
            &format!("/dists/{}/*", codename),
        ])
        .output()
        .map_err(|e| {
            ManagerError::ValidationError(format!("aws cloudfront create-invalidation: {}", e))
        })?;

    for line in String::from_utf8_lossy(&out.stdout).lines() {
        println!("    {}", line);
    }
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        eprintln!("    {}", line);
    }
    if out.status.success() {
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
}
