//! Buildkite REST adapter.
//!
//! Read-only. The REST API allows 200 requests per minute per token, so the
//! number of builds whose artifacts are fetched is bounded by the caller and
//! concurrency is capped here.

use std::path::PathBuf;

use futures::stream::{self, StreamExt};
use serde::Deserialize;

use crate::error::{OpsError, OpsResult};
use crate::model::BuildSummary;

const API_ROOT: &str = "https://api.buildkite.com/v2";
const MAX_CONCURRENT_REQUESTS: usize = 8;

/// Environment variables searched for a token, in order. The second name is
/// the one the `ci-build-me` function already uses.
const TOKEN_ENV_VARS: [&str; 2] = ["BUILDKITE_API_TOKEN", "BUILDKITE_API_ACCESS_TOKEN"];

pub struct BuildkiteClient {
    http: reqwest::Client,
    token: String,
    org: String,
    api_root: String,
}

#[derive(Debug, Deserialize)]
struct ApiBuild {
    id: String,
    number: u64,
    state: String,
    branch: String,
    commit: String,
    message: Option<String>,
    created_at: Option<String>,
    web_url: String,
    pipeline: Option<ApiPipeline>,
}

#[derive(Debug, Deserialize)]
struct ApiPipeline {
    slug: String,
}

#[derive(Debug, Deserialize)]
struct ApiArtifact {
    filename: String,
    state: String,
}

/// A build to create. `commit` accepts `HEAD`, which Buildkite resolves
/// against the branch.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NewBuild {
    pub commit: String,
    pub branch: String,
    pub message: String,
    pub env: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct CreatedBuild {
    pub number: u64,
    pub state: String,
    pub web_url: String,
}

#[derive(Debug, Deserialize)]
struct ApiPipelineBuild {
    number: u64,
    state: String,
    branch: String,
    commit: String,
    message: Option<String>,
    created_at: Option<String>,
    web_url: String,
    #[serde(default)]
    jobs: Vec<ApiJob>,
}

#[derive(Debug, Deserialize)]
struct ApiJob {
    name: Option<String>,
    step_key: Option<String>,
    state: Option<String>,
    #[serde(default)]
    soft_failed: bool,
    #[serde(default)]
    retried: bool,
    web_url: Option<String>,
    exit_status: Option<i64>,
    parallel_group_index: Option<u64>,
}

/// One build of a named pipeline, with its jobs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PipelineBuild {
    pub number: u64,
    pub state: String,
    pub branch: String,
    pub commit: String,
    pub message: Option<String>,
    pub created_at: Option<String>,
    pub web_url: String,
    pub jobs: Vec<JobSummary>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct JobSummary {
    /// Stable identity across builds: the step key when the pipeline defines
    /// one, otherwise the label. Parallel jobs carry their index, so one
    /// shard failing is not confused with another.
    pub key: String,
    pub name: String,
    pub state: String,
    /// A soft-failed job does not fail its build. It is still a real failure
    /// and is reported, but separately.
    pub soft_failed: bool,
    /// True on the superseded attempt of a retried job. Those are excluded
    /// from failure counts: the retry's outcome is the one that counts.
    pub retried: bool,
    pub web_url: Option<String>,
    pub exit_status: Option<i64>,
}

impl JobSummary {
    pub fn has_failed(&self) -> bool {
        !self.retried && matches!(self.state.as_str(), "failed" | "broken" | "waiting_failed")
    }
}

pub fn token_file_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("mina-ops")
        .join("buildkite-token")
}

/// Reads the token from the environment, then from the token file. The token
/// is never written anywhere by this tool.
pub fn token_from_environment() -> OpsResult<String> {
    for var in TOKEN_ENV_VARS {
        if let Ok(value) = std::env::var(var) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Ok(value);
            }
        }
    }
    let path = token_file_path();
    if let Ok(contents) = std::fs::read_to_string(&path) {
        let token = contents.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    Err(OpsError::MissingToken(path.display().to_string()))
}

impl BuildkiteClient {
    pub fn new(org: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: token.into(),
            org: org.into(),
            api_root: API_ROOT.to_string(),
        }
    }

    /// Point the client at another base URL. Used by the tests.
    pub fn with_api_root(mut self, root: impl Into<String>) -> Self {
        self.api_root = root.into();
        self
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, url: &str) -> OpsResult<T> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .header("User-Agent", "mina-ops")
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(OpsError::Buildkite(format!(
                "{status} from Buildkite. The token needs the read_builds and read_artifacts scopes."
            )));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(OpsError::Buildkite(
                "rate limited by Buildkite (200 requests per minute). Reduce --max-builds and retry.".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(OpsError::Buildkite(format!("{status} from {url}")));
        }
        response.json::<T>().await.map_err(OpsError::from)
    }

    /// Builds across the whole organisation for one commit.
    ///
    /// Buildkite matches the full 40-character SHA, so a short hash returns
    /// nothing. Callers should expand it first.
    pub async fn builds_for_commit(&self, commit: &str) -> OpsResult<Vec<BuildSummary>> {
        let url = format!(
            "{}/organizations/{}/builds?commit={}&per_page=100",
            self.api_root, self.org, commit
        );
        let builds: Vec<ApiBuild> = self.get(&url).await?;
        Ok(builds.into_iter().map(Into::into).collect())
    }

    /// The most recent builds of one pipeline, with their jobs.
    ///
    /// Buildkite returns jobs inline, so `count` builds and every job in them
    /// cost a single request.
    pub async fn recent_pipeline_builds(
        &self,
        pipeline: &str,
        branch: Option<&str>,
        count: usize,
    ) -> OpsResult<Vec<PipelineBuild>> {
        let branch_filter = match branch {
            Some(branch) => format!("&branch={branch}"),
            None => String::new(),
        };
        let url = format!(
            "{}/organizations/{}/pipelines/{}/builds?per_page={}{}",
            self.api_root,
            self.org,
            pipeline,
            count.clamp(1, 100),
            branch_filter
        );
        let builds: Vec<ApiPipelineBuild> = self.get(&url).await?;
        Ok(builds.into_iter().map(Into::into).collect())
    }

    /// Creates a build. The only write this tool performs.
    ///
    /// Everything after creation belongs to Buildkite: the returned URL is
    /// where the caller is sent, and no build state is rendered here.
    pub async fn create_build(
        &self,
        pipeline: &str,
        request: &NewBuild,
    ) -> OpsResult<CreatedBuild> {
        let url = format!(
            "{}/organizations/{}/pipelines/{}/builds",
            self.api_root, self.org, pipeline
        );

        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .header("User-Agent", "mina-ops")
            .json(request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(OpsError::Buildkite(format!(
                "{status} creating a build in '{pipeline}': {}",
                body.trim()
            )));
        }
        response
            .json::<CreatedBuild>()
            .await
            .map_err(OpsError::from)
    }

    /// Filenames of a build's artifacts that Buildkite still holds.
    ///
    /// Artifacts in any state other than `finished` are excluded: an expired
    /// or deleted artifact is not something that can be downloaded.
    pub async fn artifact_filenames(&self, pipeline: &str, number: u64) -> OpsResult<Vec<String>> {
        let url = format!(
            "{}/organizations/{}/pipelines/{}/builds/{}/artifacts?per_page=100",
            self.api_root, self.org, pipeline, number
        );
        let artifacts: Vec<ApiArtifact> = self.get(&url).await?;
        Ok(artifacts
            .into_iter()
            .filter(|a| a.state == "finished")
            .map(|a| a.filename)
            .collect())
    }

    /// Fills in `artifact_count` and `versions` for each build, concurrently.
    ///
    /// A build whose artifact list cannot be read keeps `artifact_count =
    /// None`, so "not asked" stays distinguishable from "none left".
    pub async fn enrich_with_artifacts(&self, builds: &mut [BuildSummary]) -> Vec<String> {
        // Owned tuples rather than borrowed builds: a stream whose items
        // borrow cannot satisfy the higher-ranked lifetime bound that callers
        // such as the HTTP server require.
        let wanted: Vec<(usize, String, u64)> = builds
            .iter()
            .enumerate()
            .map(|(index, build)| (index, build.pipeline.clone(), build.number))
            .collect();

        let results = stream::iter(wanted.into_iter().map(
            |(index, pipeline, number)| async move {
                let outcome = self.artifact_filenames(&pipeline, number).await;
                (index, pipeline, number, outcome)
            },
        ))
        .buffer_unordered(MAX_CONCURRENT_REQUESTS)
        .collect::<Vec<_>>()
        .await;

        let mut warnings = Vec::new();
        for (index, pipeline, number, outcome) in results {
            match outcome {
                Ok(filenames) => {
                    builds[index].artifact_count = Some(filenames.len());
                    builds[index].versions = versions_from_artifacts(&filenames);
                }
                Err(e) => warnings.push(format!(
                    "could not list artifacts of {pipeline}#{number}: {e}"
                )),
            }
        }
        warnings
    }
}

/// Distinct package versions parsed from `.deb` artifact filenames, in the
/// order first seen.
pub fn versions_from_artifacts(filenames: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for filename in filenames {
        if !filename.ends_with(".deb") {
            continue;
        }
        let base = filename.rsplit('/').next().unwrap_or(filename);
        if let Ok(version) = release_manager::artifacts::extract_version_from_deb(base) {
            if !out.contains(&version) {
                out.push(version);
            }
        }
    }
    out
}

impl From<ApiBuild> for BuildSummary {
    fn from(build: ApiBuild) -> Self {
        // Fall back to the slug embedded in the web URL when the pipeline
        // object is absent, so a build is never dropped for want of a name.
        let pipeline = build
            .pipeline
            .map(|p| p.slug)
            .or_else(|| pipeline_from_web_url(&build.web_url))
            .unwrap_or_else(|| "unknown".to_string());

        BuildSummary {
            pipeline,
            id: build.id,
            number: build.number,
            state: build.state,
            branch: build.branch,
            commit: build.commit,
            message: build.message.map(|m| first_line(&m)),
            created_at: build.created_at,
            web_url: build.web_url,
            artifact_count: None,
            versions: Vec::new(),
        }
    }
}

impl From<ApiPipelineBuild> for PipelineBuild {
    fn from(build: ApiPipelineBuild) -> Self {
        PipelineBuild {
            number: build.number,
            state: build.state,
            branch: build.branch,
            commit: build.commit,
            message: build.message.map(|m| first_line(&m)),
            created_at: build.created_at,
            web_url: build.web_url,
            jobs: build.jobs.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ApiJob> for JobSummary {
    fn from(job: ApiJob) -> Self {
        let label = job
            .name
            .clone()
            .or_else(|| job.step_key.clone())
            .unwrap_or_else(|| "(unnamed job)".to_string());

        // Prefer the step key: labels carry emoji and wording that change
        // between builds, which would make a long-standing failure look new.
        let base = job.step_key.clone().unwrap_or_else(|| label.clone());
        let key = match job.parallel_group_index {
            Some(index) => format!("{base}#{index}"),
            None => base,
        };

        JobSummary {
            key,
            name: label,
            state: job.state.unwrap_or_else(|| "unknown".to_string()),
            soft_failed: job.soft_failed,
            retried: job.retried,
            web_url: job.web_url,
            exit_status: job.exit_status,
        }
    }
}

fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or("").trim().to_string()
}

/// `https://buildkite.com/<org>/<pipeline>/builds/<n>` -> `<pipeline>`
fn pipeline_from_web_url(url: &str) -> Option<String> {
    let parts: Vec<&str> = url.trim_end_matches('/').split('/').collect();
    let builds_at = parts.iter().rposition(|p| *p == "builds")?;
    parts.get(builds_at.checked_sub(1)?).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_parsed_from_deb_artifacts_only() {
        let files = vec![
            "mina-devnet_3.2.0-49b523c.deb".to_string(),
            "logs/build.log".to_string(),
            "mina-archive-devnet_3.2.0-49b523c.deb".to_string(),
            "mina-logproc_3.2.0-other.deb".to_string(),
        ];
        assert_eq!(
            versions_from_artifacts(&files),
            vec!["3.2.0-49b523c".to_string(), "3.2.0-other".to_string()]
        );
    }

    #[test]
    fn versions_handle_nested_artifact_paths() {
        let files = vec!["debians/noble/mina-devnet_3.2.0-49b523c.deb".to_string()];
        assert_eq!(versions_from_artifacts(&files), vec!["3.2.0-49b523c"]);
    }

    #[test]
    fn pipeline_slug_recovered_from_web_url() {
        assert_eq!(
            pipeline_from_web_url("https://buildkite.com/o-1-labs-2/mina-nightly/builds/1579"),
            Some("mina-nightly".to_string())
        );
        assert_eq!(pipeline_from_web_url("https://example.com/"), None);
    }

    #[test]
    fn commit_message_is_reduced_to_its_first_line() {
        assert_eq!(first_line("subject\n\nbody text"), "subject");
    }
}
