use std::process::Command;

use crate::artifacts::{get_arch_suffix, get_artifact_with_suffix, get_suffix, parse_string_list};
use crate::cli::ProgressArgs;
use crate::errors::ManagerResult;

const S3_REGION: &str = "us-west-2";
const GCR_REPO: &str = "gcr.io/o1labs-192920";
const DOCKER_IO_REPO: &str = "docker.io/minaprotocol";

pub async fn execute(args: ProgressArgs) -> ManagerResult<()> {
    let artifacts = parse_string_list(&args.artifacts);
    let codenames = parse_string_list(&args.codenames);

    let network = network_for_channel(&args.release);
    let buckets: Vec<String> = buckets_for_channel(&args.release)
        .into_iter()
        .filter(|b| !(args.skip_mina_public && b.contains("packages.minaprotocol.com")))
        .collect();

    println!();
    print_header();
    println!(" 📦  Version: {}", args.version);
    println!(" 🏷️   Release: {}", args.release);
    println!(" 🌐  Network: {}", network);
    println!(" 📚  Artifacts: {}", args.artifacts);
    println!(" 🖥️   Codenames: {}", args.codenames);
    print_header();
    println!();

    let mut totals = Totals::default();

    if !args.only_dockers {
        println!("📦 DEBIAN PACKAGES");
        print_header();
        println!();
        for bucket in &buckets {
            println!("  🗄️  Repository: {}", bucket);
            println!("  ─────────────────────────────────────────────────────────────");
            println!();

            for codename in &codenames {
                for arch in archs_for_codename(codename) {
                    println!("    📋  Checking {}/{}...", codename, arch);
                    let available = deb_s3_list(bucket, &args.release, codename, arch);

                    for artifact in &artifacts {
                        check_artifact_in_debs3(
                            artifact,
                            &available,
                            arch,
                            &args.version,
                            &network,
                            args.profile.as_deref(),
                            &mut totals,
                        );
                    }
                }
            }
            println!();
        }
    }

    if !args.only_debians {
        println!("🐋 DOCKER IMAGES");
        print_header();
        println!();
        let docker_repo = if args.release == "stable" {
            DOCKER_IO_REPO
        } else {
            GCR_REPO
        };
        println!("  🐳  Registry: {}", docker_repo);
        println!("  ─────────────────────────────────────────────────────────────");
        println!();

        for artifact in &artifacts {
            if !artifact_has_docker(artifact) {
                continue;
            }
            for codename in &codenames {
                for arch in archs_for_codename(codename) {
                    let net_suffix = get_suffix(artifact, Some(&network), args.profile.as_deref());
                    let arch_suffix = get_arch_suffix(arch);
                    let tag = format!("{}-{}{}{}", args.version, codename, net_suffix, arch_suffix);

                    totals.docker_total += 1;
                    if check_docker_manifest(docker_repo, artifact, &tag) {
                        println!("    ✅  {}:{}", artifact, tag);
                        totals.docker_passed += 1;
                    } else {
                        println!("    ❌  {}:{} - MISSING", artifact, tag);
                    }
                }
            }
        }
        println!();
    }

    print_summary(&args, &totals);
    Ok(())
}

#[derive(Default, Debug)]
struct Totals {
    debian_total: usize,
    debian_passed: usize,
    docker_total: usize,
    docker_passed: usize,
}

fn print_header() {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

fn network_for_channel(channel: &str) -> String {
    match channel {
        "alpha" => "devnet",
        "beta" | "stable" => "mainnet",
        _ => "mainnet",
    }
    .to_string()
}

fn buckets_for_channel(channel: &str) -> Vec<String> {
    match channel {
        "alpha" | "beta" => vec![
            "unstable.apt.packages.minaprotocol.com".to_string(),
            "packages.o1test.net".to_string(),
        ],
        "stable" => vec![
            "stable.apt.packages.minaprotocol.com".to_string(),
            "packages.o1test.net".to_string(),
        ],
        _ => vec!["packages.o1test.net".to_string()],
    }
}

fn archs_for_codename(codename: &str) -> &'static [&'static str] {
    match codename {
        "bookworm" | "noble" => &["amd64", "arm64"],
        _ => &["amd64"],
    }
}

fn artifact_has_docker(artifact: &str) -> bool {
    !matches!(
        artifact,
        "mina-logproc"
            | "minimina"
            | "mina-config"
            | "mina-automode"
            | "mina-prefork"
            | "mina-postfork"
            | "mina-postfork-mesa"
            | "mina-prefork-mesa"
    )
}

fn deb_s3_list(bucket: &str, component: &str, codename: &str, arch: &str) -> String {
    let output = Command::new("deb-s3")
        .args([
            "list",
            &format!("--bucket={}", bucket),
            &format!("--s3-region={}", S3_REGION),
            "--component",
            component,
            "--codename",
            codename,
            "--arch",
            arch,
        ])
        .output();
    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(_) => String::new(),
    }
}

/// `deb-s3 list` prints lines like `mina-daemon 1.0.0-bullseye-devnet amd64 …`.
/// We check that `<name> <version> <arch>` appears as the first three fields.
fn package_present(listing: &str, name: &str, version: &str, arch: &str) -> bool {
    listing.lines().any(|line| {
        let mut parts = line.split_whitespace();
        let n = parts.next().unwrap_or("");
        let v = parts.next().unwrap_or("");
        let a = parts.next().unwrap_or("");
        n == name && v == version && a == arch
    })
}

fn check_artifact_in_debs3(
    artifact: &str,
    available: &str,
    arch: &str,
    version: &str,
    network: &str,
    profile: Option<&str>,
    totals: &mut Totals,
) {
    match artifact {
        "mina-logproc" | "minimina" => {
            check_one(artifact, version, arch, available, totals);
        }
        "mina-archive" => {
            let with_suffix = get_artifact_with_suffix(artifact, Some(network), None);
            check_one(&with_suffix, version, arch, available, totals);
            // For non-devnet, also check unsuffixed package name
            if network != "devnet" {
                check_one(artifact, version, arch, available, totals);
            }
        }
        "mina-config" => {
            let with_suffix = get_artifact_with_suffix(artifact, Some(network), None);
            check_one(&with_suffix, version, "all", available, totals);
        }
        "mina-daemon" | "mina-rosetta" | "mina-generic" | "rosetta-generic"
        | "mina-postfork-mesa" | "mina-prefork-mesa" => {
            let with_suffix = get_artifact_with_suffix(artifact, Some(network), profile);
            check_one(&with_suffix, version, arch, available, totals);
        }
        "mina-automode" | "mina-prefork" | "mina-postfork" => {
            let with_suffix = get_artifact_with_suffix(artifact, Some(network), None);
            check_one(&with_suffix, version, arch, available, totals);
        }
        _ => { /* unknown artifact — silently ignore */ }
    }
}

fn check_one(name: &str, version: &str, arch: &str, available: &str, totals: &mut Totals) {
    totals.debian_total += 1;
    if package_present(available, name, version, arch) {
        println!("      ✅  {}", name);
        totals.debian_passed += 1;
    } else {
        println!("      ❌  {} - MISSING", name);
    }
}

fn check_docker_manifest(repo: &str, artifact: &str, tag: &str) -> bool {
    let target = format!("{}/{}:{}", repo, artifact, tag);
    Command::new("docker")
        .args(["manifest", "inspect", &target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn progress_bar(passed: usize, total: usize) -> String {
    let total = total.max(1);
    let bar_length = 50;
    let filled = passed * bar_length / total;
    let mut s = String::from("      [");
    for i in 0..bar_length {
        s.push(if i < filled { '█' } else { '░' });
    }
    s.push(']');
    s
}

fn print_summary(args: &ProgressArgs, t: &Totals) {
    print_header();
    println!("📊 SUMMARY");
    print_header();
    println!();

    if !args.only_dockers {
        let pct = t.debian_passed * 100 / t.debian_total.max(1);
        println!(
            "  📦  Debian Packages: {} / {} ({}%)",
            t.debian_passed, t.debian_total, pct
        );
        println!("{}", progress_bar(t.debian_passed, t.debian_total));
        println!();
    }

    if !args.only_debians {
        let pct = t.docker_passed * 100 / t.docker_total.max(1);
        println!(
            "  🐋  Docker Images: {} / {} ({}%)",
            t.docker_passed, t.docker_total, pct
        );
        println!("{}", progress_bar(t.docker_passed, t.docker_total));
        println!();
    }

    let total = t.debian_total + t.docker_total;
    let passed = t.debian_passed + t.docker_passed;
    if total > 0 {
        let pct = passed * 100 / total;
        println!("  🎯  Overall Progress: {} / {} ({}%)", passed, total, pct);
        println!("{}", progress_bar(passed, total));
        println!();
    }

    if passed == total && total > 0 {
        println!("  🎉  Congratulations! All artifacts are published!");
    } else {
        println!("  ⚠️   There are missing artifacts. Please review the list above.");
    }

    println!();
    print_header();
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_mapping() {
        assert_eq!(network_for_channel("alpha"), "devnet");
        assert_eq!(network_for_channel("beta"), "mainnet");
        assert_eq!(network_for_channel("stable"), "mainnet");
        assert_eq!(network_for_channel("unknown"), "mainnet");
    }

    #[test]
    fn bucket_mapping() {
        assert!(buckets_for_channel("alpha")
            .iter()
            .any(|b| b.contains("unstable")));
        assert!(buckets_for_channel("stable")
            .iter()
            .any(|b| b.contains("stable")));
        assert_eq!(buckets_for_channel("dev").len(), 1);
    }

    #[test]
    fn arch_per_codename() {
        assert_eq!(archs_for_codename("bullseye"), &["amd64"]);
        assert_eq!(archs_for_codename("bookworm"), &["amd64", "arm64"]);
        assert_eq!(archs_for_codename("noble"), &["amd64", "arm64"]);
    }

    #[test]
    fn package_present_basic() {
        let listing = "mina-daemon 1.0.0-bullseye-devnet amd64 some-extra\n\
                       mina-archive 1.0.0-bullseye-devnet amd64\n";
        assert!(package_present(
            listing,
            "mina-daemon",
            "1.0.0-bullseye-devnet",
            "amd64"
        ));
        assert!(!package_present(
            listing,
            "mina-daemon",
            "wrong-version",
            "amd64"
        ));
        assert!(!package_present(
            listing,
            "mina-daemon",
            "1.0.0-bullseye-devnet",
            "arm64"
        ));
    }

    #[test]
    fn artifacts_without_docker() {
        assert!(!artifact_has_docker("mina-logproc"));
        assert!(!artifact_has_docker("minimina"));
        assert!(!artifact_has_docker("mina-config"));
        assert!(!artifact_has_docker("mina-automode"));
        assert!(!artifact_has_docker("mina-prefork-mesa"));
        assert!(artifact_has_docker("mina-daemon"));
        assert!(artifact_has_docker("mina-generic"));
    }
}
