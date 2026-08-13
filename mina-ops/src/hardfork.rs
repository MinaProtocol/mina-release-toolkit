//! Hardfork package-generation parameters, and the checks that make them
//! safe to submit.
//!
//! These parameters are otherwise pasted by hand into Buildkite as a block of
//! environment variables. A wrong codename fails in seconds, but a URL that
//! does not exist, or a build UUID whose packages have been pruned, fails
//! after a long build. Every check here is cheap and runs before the build is
//! created.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::adapters::hetzner::{self, CacheClient};
use crate::config::Project;
use crate::model::Presence;

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

/// One field's worth of parameters, as the pipeline expects them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardforkParams {
    /// Comma-separated `<Codename>_<Arch>`, for example `Jammy_Amd64`.
    pub codenames_config: String,
    pub network: String,
    pub genesis_timestamp: String,
    pub config_json_gz_url: String,
    #[serde(default)]
    pub precomputed_fork_block_prefix: Option<String>,
    #[serde(default)]
    pub use_artifacts_from_buildkite_build: Option<String>,
    #[serde(default)]
    pub use_generic_dockers_from_version: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub mina_ledger_s3_bucket: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Branch the pipeline runs from.
    pub branch: String,
}

impl HardforkParams {
    /// The environment block Buildkite receives — the same one that would
    /// otherwise be pasted by hand.
    pub fn to_env(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert("CODENAMES_CONFIG".into(), self.codenames_config.clone());
        env.insert("NETWORK".into(), self.network.clone());
        env.insert("GENESIS_TIMESTAMP".into(), self.genesis_timestamp.clone());
        env.insert("CONFIG_JSON_GZ_URL".into(), self.config_json_gz_url.clone());
        // Large repositories are cloned without LFS content, as the pipeline
        // has always done.
        env.insert("GIT_LFS_SKIP_SMUDGE".into(), "1".into());

        let mut optional = |key: &str, value: &Option<String>| {
            if let Some(value) = value.as_ref().map(|v| v.trim()).filter(|v| !v.is_empty()) {
                env.insert(key.to_string(), value.to_string());
            }
        };
        optional(
            "PRECOMPUTED_FORK_BLOCK_PREFIX",
            &self.precomputed_fork_block_prefix,
        );
        optional(
            "USE_ARTIFACTS_FROM_BUILDKITE_BUILD",
            &self.use_artifacts_from_buildkite_build,
        );
        optional(
            "USE_GENERIC_DOCKERS_FROM_VERSION",
            &self.use_generic_dockers_from_version,
        );
        optional("REPO", &self.repo);
        optional("MINA_LEDGER_S3_BUCKET", &self.mina_ledger_s3_bucket);
        optional("VERSION", &self.version);
        env
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "detail", rename_all = "snake_case")]
pub enum Outcome {
    Passed(String),
    /// The value is wrong. Submitting would waste a build.
    Failed(String),
    /// The check could not run. Not a reason to block, but it is reported.
    Unknown(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub field: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}

impl Check {
    fn passed(field: &str, detail: impl Into<String>) -> Self {
        Check {
            field: field.to_string(),
            outcome: Outcome::Passed(detail.into()),
        }
    }
    fn failed(field: &str, detail: impl Into<String>) -> Self {
        Check {
            field: field.to_string(),
            outcome: Outcome::Failed(detail.into()),
        }
    }
    fn unknown(field: &str, detail: impl Into<String>) -> Self {
        Check {
            field: field.to_string(),
            outcome: Outcome::Unknown(detail.into()),
        }
    }
    pub fn has_failed(&self) -> bool {
        matches!(self.outcome, Outcome::Failed(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validation {
    pub checks: Vec<Check>,
    /// The environment block that would be sent, shown before anything is
    /// created so it can be read — or copied into the Buildkite UI instead.
    pub env: BTreeMap<String, String>,
    pub pipeline: String,
}

impl Validation {
    pub fn is_submittable(&self) -> bool {
        !self.checks.iter().any(Check::has_failed)
    }
}

/// Checks that need no network. Kept separate so they can be tested without
/// reaching anything.
pub fn check_offline(params: &HardforkParams) -> Vec<Check> {
    let mut checks = Vec::new();

    checks.push(check_codenames(&params.codenames_config));

    checks.push(if NETWORKS.contains(&params.network.as_str()) {
        Check::passed("NETWORK", &params.network)
    } else {
        Check::failed("NETWORK", format!("must be one of {}", NETWORKS.join(", ")))
    });

    checks.push(check_timestamp(&params.genesis_timestamp));

    if let Some(repo) = params.repo.as_deref().filter(|r| !r.is_empty()) {
        checks.push(if REPOS.contains(&repo) {
            Check::passed("REPO", repo)
        } else {
            Check::failed("REPO", format!("must be one of {}", REPOS.join(", ")))
        });
    }

    if let Some(build) = params
        .use_artifacts_from_buildkite_build
        .as_deref()
        .filter(|b| !b.is_empty())
    {
        checks.push(if looks_like_uuid(build) {
            Check::passed("USE_ARTIFACTS_FROM_BUILDKITE_BUILD", "well-formed UUID")
        } else {
            Check::failed(
                "USE_ARTIFACTS_FROM_BUILDKITE_BUILD",
                "must be a Buildkite build UUID, not a build number",
            )
        });
    }

    if params.branch.trim().is_empty() {
        checks.push(Check::failed("branch", "a branch is required"));
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

/// Checks that reach the outside world: does the config exist, are the
/// referenced build's packages still cached.
pub async fn check_online(params: &HardforkParams, project: &Project) -> Vec<Check> {
    let mut checks = Vec::new();

    checks.push(check_url(&params.config_json_gz_url).await);

    if let Some(build_id) = params
        .use_artifacts_from_buildkite_build
        .as_deref()
        .filter(|b| looks_like_uuid(b))
    {
        checks.push(check_cached_build(build_id, project).await);
    }

    if let Some(prefix) = params
        .precomputed_fork_block_prefix
        .as_deref()
        .filter(|p| !p.is_empty())
    {
        checks.push(check_gs_prefix(prefix).await);
    }

    checks
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

pub async fn validate(params: &HardforkParams, project: &Project, pipeline: &str) -> Validation {
    let mut checks = check_offline(params);
    checks.extend(check_online(params, project).await);
    Validation {
        checks,
        env: params.to_env(),
        pipeline: pipeline.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> HardforkParams {
        HardforkParams {
            codenames_config: "Jammy_Amd64".into(),
            network: "Devnet".into(),
            genesis_timestamp: "2026-08-08T21:00:00.00Z".into(),
            config_json_gz_url: "https://storage.googleapis.com/bucket/config.json.gz".into(),
            precomputed_fork_block_prefix: Some("gs://mesa-hf-precomputed-blocks/x".into()),
            use_artifacts_from_buildkite_build: Some("019fe333-ee52-45fa-8404-ba189bc1b57b".into()),
            use_generic_dockers_from_version: Some("4.0.0-rc1-devnet-dryrun-ee5cb7f".into()),
            repo: Some("Nightly".into()),
            mina_ledger_s3_bucket: Some(
                "https://s3-us-west-2.amazonaws.com/snark-keys.o1test.net".into(),
            ),
            version: None,
            branch: "devnet-dryrun".into(),
        }
    }

    fn failures(checks: &[Check]) -> Vec<&str> {
        checks
            .iter()
            .filter(|c| c.has_failed())
            .map(|c| c.field.as_str())
            .collect()
    }

    #[test]
    fn a_realistic_parameter_set_passes_the_offline_checks() {
        assert!(failures(&check_offline(&params())).is_empty());
    }

    #[test]
    fn the_environment_block_matches_what_the_pipeline_expects() {
        let env = params().to_env();
        assert_eq!(env.get("CODENAMES_CONFIG").unwrap(), "Jammy_Amd64");
        assert_eq!(env.get("GIT_LFS_SKIP_SMUDGE").unwrap(), "1");
        assert_eq!(
            env.get("USE_ARTIFACTS_FROM_BUILDKITE_BUILD").unwrap(),
            "019fe333-ee52-45fa-8404-ba189bc1b57b"
        );
        // An unset optional is absent, not empty: the pipeline's script tests
        // for emptiness to decide whether the value was given.
        assert!(!env.contains_key("VERSION"));
    }

    #[test]
    fn blank_optionals_are_left_out_entirely() {
        let mut p = params();
        p.version = Some("   ".into());
        assert!(!p.to_env().contains_key("VERSION"));
    }

    #[test]
    fn a_lowercase_codename_is_rejected() {
        let mut p = params();
        p.codenames_config = "jammy_Amd64".into();
        assert_eq!(failures(&check_offline(&p)), vec!["CODENAMES_CONFIG"]);
    }

    #[test]
    fn an_unknown_architecture_is_rejected() {
        let mut p = params();
        p.codenames_config = "Jammy_Riscv".into();
        assert_eq!(failures(&check_offline(&p)), vec!["CODENAMES_CONFIG"]);
    }

    #[test]
    fn several_codename_entries_are_accepted() {
        let mut p = params();
        p.codenames_config = "Jammy_Amd64, Noble_Arm64".into();
        assert!(failures(&check_offline(&p)).is_empty());
    }

    #[test]
    fn a_timestamp_without_z_is_rejected() {
        let mut p = params();
        p.genesis_timestamp = "2026-08-08T21:00:00.00".into();
        assert_eq!(failures(&check_offline(&p)), vec!["GENESIS_TIMESTAMP"]);
    }

    #[test]
    fn an_unparseable_timestamp_is_rejected() {
        let mut p = params();
        p.genesis_timestamp = "not-a-date-Z".into();
        assert_eq!(failures(&check_offline(&p)), vec!["GENESIS_TIMESTAMP"]);
    }

    #[test]
    fn a_build_number_in_place_of_a_uuid_is_rejected() {
        let mut p = params();
        p.use_artifacts_from_buildkite_build = Some("1305".into());
        assert_eq!(
            failures(&check_offline(&p)),
            vec!["USE_ARTIFACTS_FROM_BUILDKITE_BUILD"]
        );
    }

    #[test]
    fn uuid_recognition() {
        assert!(looks_like_uuid("019fe333-ee52-45fa-8404-ba189bc1b57b"));
        assert!(!looks_like_uuid("019fe333ee5245fa8404ba189bc1b57b"));
        assert!(!looks_like_uuid("zzzfe333-ee52-45fa-8404-ba189bc1b57b"));
        assert!(!looks_like_uuid(""));
    }

    #[test]
    fn an_unknown_network_or_repo_is_rejected() {
        let mut p = params();
        p.network = "Testnet".into();
        p.repo = Some("Wherever".into());
        let checks = check_offline(&p);
        let failed = failures(&checks);
        assert!(failed.contains(&"NETWORK"));
        assert!(failed.contains(&"REPO"));
    }

    #[test]
    fn a_validation_with_any_failure_is_not_submittable() {
        let validation = Validation {
            checks: vec![
                Check::passed("a", "fine"),
                Check::unknown("b", "could not check"),
            ],
            env: BTreeMap::new(),
            pipeline: "p".into(),
        };
        assert!(validation.is_submittable(), "unknown alone must not block");

        let blocked = Validation {
            checks: vec![Check::failed("a", "wrong")],
            env: BTreeMap::new(),
            pipeline: "p".into(),
        };
        assert!(!blocked.is_submittable());
    }
}
