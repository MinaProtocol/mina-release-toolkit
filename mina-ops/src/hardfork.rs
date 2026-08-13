//! Hardfork package-generation parameters, and the checks that make them
//! safe to submit.
//!
//! These parameters are otherwise pasted by hand into Buildkite as a block of
//! environment variables. A wrong codename fails in seconds, but a URL that
//! does not exist, or a build UUID whose packages have been pruned, fails
//! after a long build. Every check here is cheap and runs before the build is
//! created.

use std::collections::BTreeMap;

use crate::adapters::hetzner::{self, CacheClient};
use crate::config::Project;
use crate::model::Presence;
use crate::pipelines::Check;

/// Codenames and architectures the pipeline's Dhall accepts, in the
/// capitalised spelling `CODENAMES_CONFIG` requires.
///
/// This list mirrors `buildkite/src/Constants/DebianVersions.dhall` in the
/// mina repository. It is the one piece of knowledge here that can drift; a
/// wrong value costs seconds, because the Dhall fails immediately.
pub const CODENAMES: [&str; 5] = ["Bullseye", "Bookworm", "Focal", "Jammy", "Noble"];
pub const ARCHITECTURES: [&str; 2] = ["Amd64", "Arm64"];
pub const NETWORKS: [&str; 2] = ["Devnet", "Mainnet"];
pub const REPOS: [&str; 4] = ["Nightly", "Unstable", "Alpha", "Beta"];

/// Deep checks for the hardfork pipeline: the ones that reach the outside
/// world and cannot be expressed as a pattern or a list of options.
///
/// A wrong codename fails in seconds because the pipeline's Dhall rejects it.
/// A config URL that does not exist, or a build whose packages have been
/// pruned, fails after a long build — which is what these prevent.
pub async fn deep_checks(env: &BTreeMap<String, String>, project: &Project) -> Vec<Check> {
    let mut checks = Vec::new();

    if let Some(url) = env.get("CONFIG_JSON_GZ_URL") {
        checks.push(check_url(url).await);
    }

    if let Some(build_id) = env
        .get("USE_ARTIFACTS_FROM_BUILDKITE_BUILD")
        .filter(|b| looks_like_uuid(b))
    {
        checks.push(check_cached_build(build_id, project).await);
    }

    if let Some(prefix) = env.get("PRECOMPUTED_FORK_BLOCK_PREFIX") {
        checks.push(check_gs_prefix(prefix).await);
    }

    if let Some(codenames) = env.get("CODENAMES_CONFIG") {
        checks.push(check_codenames(codenames));
    }

    if let Some(timestamp) = env.get("GENESIS_TIMESTAMP") {
        checks.push(check_timestamp(timestamp));
    }

    checks
}

fn check_codenames(value: &str) -> Check {
    let entries: Vec<&str> = value
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect();
    if entries.is_empty() {
        return Check::failed("CODENAMES_CONFIG", "at least one entry is required");
    }

    for entry in &entries {
        let Some((codename, arch)) = entry.split_once('_') else {
            return Check::failed(
                "CODENAMES_CONFIG",
                format!("'{entry}' must be written <Codename>_<Arch>, for example Jammy_Amd64"),
            );
        };
        if !CODENAMES.contains(&codename) {
            return Check::failed(
                "CODENAMES_CONFIG",
                format!("'{codename}' is not one of {}", CODENAMES.join(", ")),
            );
        }
        if !ARCHITECTURES.contains(&arch) {
            return Check::failed(
                "CODENAMES_CONFIG",
                format!("'{arch}' is not one of {}", ARCHITECTURES.join(", ")),
            );
        }
    }
    Check::passed("CODENAMES_CONFIG", format!("{} entries", entries.len()))
}

/// The timestamp must be UTC. A local-time value makes the automatic and the
/// legacy hardfork paths disagree, which is why the pipeline's own script
/// rejects anything not ending in `Z`.
fn check_timestamp(value: &str) -> Check {
    if value.trim().is_empty() {
        return Check::failed("GENESIS_TIMESTAMP", "a timestamp is required");
    }
    if !value.ends_with('Z') {
        return Check::failed(
            "GENESIS_TIMESTAMP",
            "must be UTC and end with 'Z', otherwise the automatic and legacy paths disagree",
        );
    }
    match chrono::DateTime::parse_from_rfc3339(value) {
        Ok(parsed) => Check::passed("GENESIS_TIMESTAMP", parsed.to_rfc3339()),
        Err(e) => Check::failed("GENESIS_TIMESTAMP", format!("not a valid timestamp: {e}")),
    }
}

pub fn looks_like_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.chars().all(|c| c.is_ascii_hexdigit()))
}

async fn check_url(url: &str) -> Check {
    let field = "CONFIG_JSON_GZ_URL";
    if url.trim().is_empty() {
        return Check::failed(field, "a URL is required");
    }
    if !url.starts_with("https://") {
        return Check::failed(field, "must be an https URL");
    }

    let client = reqwest::Client::new();
    match client.head(url).send().await {
        Ok(response) if response.status().is_success() => {
            let size = response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown size");
            Check::passed(field, format!("reachable, {size} bytes"))
        }
        Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
            Check::failed(field, "the object does not exist (404)")
        }
        Ok(response) => Check::unknown(field, format!("unexpected reply: {}", response.status())),
        Err(e) => Check::unknown(field, format!("could not be reached: {e}")),
    }
}

async fn check_cached_build(build_id: &str, project: &Project) -> Check {
    let field = "USE_ARTIFACTS_FROM_BUILDKITE_BUILD";
    let Some(route) = hetzner::resolve_route(project.cache.as_ref()) else {
        return Check::unknown(
            field,
            "no CI cache configured, so its packages were not checked",
        );
    };

    let (presence, debians) = CacheClient::new(route).debians_for_build(build_id).await;
    match presence {
        Presence::Present => Check::passed(field, format!("{} packages cached", debians.len())),
        Presence::Missing => Check::failed(
            field,
            "no packages are cached for this build; they were never written or have been pruned",
        ),
        Presence::Unknown(reason) => Check::unknown(field, reason),
    }
}

async fn check_gs_prefix(prefix: &str) -> Check {
    let field = "PRECOMPUTED_FORK_BLOCK_PREFIX";
    if !prefix.starts_with("gs://") {
        return Check::failed(field, "must start with gs://");
    }

    let output = tokio::process::Command::new("gcloud")
        .args([
            "storage",
            "ls",
            &format!("{}/", prefix.trim_end_matches('/')),
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let count = String::from_utf8_lossy(&out.stdout).lines().count();
            if count == 0 {
                Check::failed(field, "the prefix exists but holds nothing")
            } else {
                Check::passed(field, format!("{count} objects"))
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("not found") || stderr.contains("does not exist") {
                Check::failed(field, "the prefix does not exist")
            } else {
                Check::unknown(field, format!("gcloud failed: {}", first_line(&stderr)))
            }
        }
        Err(e) => Check::unknown(field, format!("gcloud is not usable: {e}")),
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("unknown error")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn failed(checks: &[Check]) -> Vec<&str> {
        checks
            .iter()
            .filter(|c| c.has_failed())
            .map(|c| c.field.as_str())
            .collect()
    }

    #[test]
    fn a_realistic_codename_config_passes() {
        let checks = vec![check_codenames("Jammy_Amd64, Noble_Arm64")];
        assert!(failed(&checks).is_empty());
    }

    #[test]
    fn a_lowercase_codename_is_rejected() {
        assert!(check_codenames("jammy_Amd64").has_failed());
    }

    #[test]
    fn an_unknown_architecture_is_rejected() {
        assert!(check_codenames("Jammy_Riscv").has_failed());
    }

    #[test]
    fn a_codename_without_an_architecture_is_rejected() {
        assert!(check_codenames("Jammy").has_failed());
        assert!(check_codenames("").has_failed());
    }

    #[test]
    fn a_timestamp_must_be_utc_and_parse() {
        assert!(!check_timestamp("2026-08-08T21:00:00.00Z").has_failed());
        // Local time makes the automatic and legacy paths disagree.
        assert!(check_timestamp("2026-08-08T21:00:00.00").has_failed());
        assert!(check_timestamp("not-a-date-Z").has_failed());
        assert!(check_timestamp("").has_failed());
    }

    #[test]
    fn uuid_recognition() {
        assert!(looks_like_uuid("019fe333-ee52-45fa-8404-ba189bc1b57b"));
        assert!(!looks_like_uuid("1305"));
        assert!(!looks_like_uuid("019fe333ee5245fa8404ba189bc1b57b"));
        assert!(!looks_like_uuid(""));
    }

    #[tokio::test]
    async fn deep_checks_only_speak_about_the_fields_present() {
        // No network is touched: none of these keys are set.
        let checks = deep_checks(&env(&[("NETWORK", "Devnet")]), &test_project()).await;
        assert!(checks.is_empty());
    }

    #[tokio::test]
    async fn deep_checks_cover_the_codenames_and_timestamp_offline() {
        let checks = deep_checks(
            &env(&[
                ("CODENAMES_CONFIG", "Jammy_Amd64"),
                ("GENESIS_TIMESTAMP", "2026-08-08T21:00:00.00Z"),
            ]),
            &test_project(),
        )
        .await;
        assert_eq!(checks.len(), 2);
        assert!(failed(&checks).is_empty());
    }

    fn test_project() -> Project {
        crate::config::Registry::builtin()
            .unwrap()
            .project("mina")
            .unwrap()
            .clone()
    }
}
