//! Nightly triage.
//!
//! A failing nightly build tells you what is red. It does not tell you what
//! is *newly* red, and that is the question triage actually starts with: which
//! of tonight's failures appeared tonight, and which have been failing for
//! days. Answering it by hand means opening several builds side by side.
//!
//! This module compares consecutive builds of one pipeline and labels every
//! failure of the newest build as new, persistent, or — when there is nothing
//! to compare against — unknown.

use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::adapters::buildkite::{token_from_environment, BuildkiteClient, JobSummary};
use crate::adapters::github::{GithubClient, PullRequest};
use crate::config::Project;
use crate::error::OpsResult;

const MAX_CONCURRENT_LOOKUPS: usize = 4;

#[derive(Debug, Clone)]
pub struct NightlyQuery {
    pub pipeline: String,
    pub branch: Option<String>,
    pub last: usize,
    pub skip_github: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailedJob {
    pub key: String,
    pub name: String,
    /// A soft failure does not turn the build red, but it is still a
    /// regression, so it is reported and marked rather than hidden.
    pub soft_failed: bool,
    pub web_url: Option<String>,
    pub exit_status: Option<i64>,
}

impl From<&JobSummary> for FailedJob {
    fn from(job: &JobSummary) -> Self {
        FailedJob {
            key: job.key.clone(),
            name: job.name.clone(),
            soft_failed: job.soft_failed,
            web_url: job.web_url.clone(),
            exit_status: job.exit_status,
        }
    }
}

/// How long a failure has been present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "trend", rename_all = "snake_case")]
pub enum Trend {
    /// Passing, or absent, in the previous build.
    New,
    /// Failing in the previous build as well. `consecutive` counts the builds
    /// examined here in which it failed, so it is a floor, not a total: the
    /// failure may be older than the window.
    Persistent { consecutive: usize },
    /// Only one build was available, so nothing can be said about the trend.
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifiedFailure {
    #[serde(flatten)]
    pub job: FailedJob,
    #[serde(flatten)]
    pub trend: Trend,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NightlyBuildSummary {
    pub number: u64,
    pub state: String,
    pub branch: String,
    pub commit: String,
    pub message: Option<String>,
    pub created_at: Option<String>,
    pub web_url: String,
    pub failures: Vec<FailedJob>,
    pub job_count: usize,
    /// Pull requests containing this build's commit — who owns the change.
    #[serde(default)]
    pub pull_requests: Vec<PullRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NightlyReport {
    pub project: String,
    pub pipeline: String,
    pub branch: Option<String>,
    pub builds: Vec<NightlyBuildSummary>,
    /// Failures of the newest build, each with its trend.
    pub latest_failures: Vec<ClassifiedFailure>,
    /// Failing in the previous build and passing in the newest one.
    pub fixed_since_previous: Vec<FailedJob>,
    pub warnings: Vec<String>,
}

impl NightlyReport {
    pub fn new_failures(&self) -> Vec<&ClassifiedFailure> {
        self.latest_failures
            .iter()
            .filter(|f| f.trend == Trend::New)
            .collect()
    }

    pub fn persistent_failures(&self) -> Vec<&ClassifiedFailure> {
        self.latest_failures
            .iter()
            .filter(|f| matches!(f.trend, Trend::Persistent { .. }))
            .collect()
    }
}

/// Failing jobs of one build, with retried attempts excluded.
pub fn failures_of(jobs: &[JobSummary]) -> Vec<FailedJob> {
    jobs.iter()
        .filter(|job| job.has_failed())
        .map(FailedJob::from)
        .collect()
}

/// Labels the newest build's failures, and lists what recovered.
///
/// `builds` must be ordered newest first.
pub fn classify(builds: &[NightlyBuildSummary]) -> (Vec<ClassifiedFailure>, Vec<FailedJob>) {
    let Some(latest) = builds.first() else {
        return (Vec::new(), Vec::new());
    };

    let classified = latest
        .failures
        .iter()
        .map(|failure| {
            let trend = if builds.len() < 2 {
                Trend::Unknown
            } else {
                // Count how many builds, from the newest backwards, failed
                // this job without interruption.
                let consecutive = builds
                    .iter()
                    .take_while(|build| build.failures.iter().any(|f| f.key == failure.key))
                    .count();
                if consecutive < 2 {
                    Trend::New
                } else {
                    Trend::Persistent { consecutive }
                }
            };
            ClassifiedFailure {
                job: failure.clone(),
                trend,
            }
        })
        .collect();

    let fixed = match builds.get(1) {
        Some(previous) => previous
            .failures
            .iter()
            .filter(|f| !latest.failures.iter().any(|l| l.key == f.key))
            .cloned()
            .collect(),
        None => Vec::new(),
    };

    (classified, fixed)
}

pub async fn collect(
    project_name: &str,
    project: &Project,
    query: &NightlyQuery,
) -> OpsResult<NightlyReport> {
    let mut warnings: Vec<String> = Vec::new();

    let token = token_from_environment()?;
    let client = BuildkiteClient::new(&project.buildkite.org, token);
    let builds = client
        .recent_pipeline_builds(&query.pipeline, query.branch.as_deref(), query.last)
        .await?;

    let mut summaries: Vec<NightlyBuildSummary> = builds
        .iter()
        .map(|build| NightlyBuildSummary {
            number: build.number,
            state: build.state.clone(),
            branch: build.branch.clone(),
            commit: build.commit.clone(),
            message: build.message.clone(),
            created_at: build.created_at.clone(),
            web_url: build.web_url.clone(),
            failures: failures_of(&build.jobs),
            job_count: build.jobs.len(),
            pull_requests: Vec::new(),
        })
        .collect();

    match summaries.len() {
        0 => warnings.push(format!(
            "no builds found for pipeline '{}'{}",
            query.pipeline,
            query
                .branch
                .as_deref()
                .map(|b| format!(" on branch '{b}'"))
                .unwrap_or_default()
        )),
        1 => warnings.push(
            "only one build was available, so no failure can be called new or persistent"
                .to_string(),
        ),
        _ => {}
    }

    if !query.skip_github {
        let github_warnings = attach_pull_requests(project, &mut summaries).await;
        warnings.extend(github_warnings);
    } else {
        warnings.push("GitHub was not queried (--skip-github)".to_string());
    }

    let (latest_failures, fixed_since_previous) = classify(&summaries);

    Ok(NightlyReport {
        project: project_name.to_string(),
        pipeline: query.pipeline.clone(),
        branch: query.branch.clone(),
        builds: summaries,
        latest_failures,
        fixed_since_previous,
        warnings,
    })
}

async fn attach_pull_requests(
    project: &Project,
    summaries: &mut [NightlyBuildSummary],
) -> Vec<String> {
    let client = GithubClient::new();
    if let Err(reason) = client.available().await {
        return vec![format!("pull requests not resolved: {reason}")];
    }

    let commits: Vec<String> = summaries.iter().map(|s| s.commit.clone()).collect();
    let results = stream::iter(commits.into_iter().enumerate().map(|(index, commit)| {
        let client = &client;
        let repo = project.repo.clone();
        async move {
            let outcome = client.pulls_for_commit(&repo, &commit).await;
            (index, outcome)
        }
    }))
    .buffer_unordered(MAX_CONCURRENT_LOOKUPS)
    .collect::<Vec<_>>()
    .await;

    let mut warnings = Vec::new();
    for (index, outcome) in results {
        match outcome {
            Ok(pulls) => summaries[index].pull_requests = pulls,
            Err(e) => warnings.push(e.to_string()),
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(key: &str, state: &str) -> JobSummary {
        JobSummary {
            key: key.to_string(),
            name: format!("label for {key}"),
            state: state.to_string(),
            soft_failed: false,
            retried: false,
            web_url: None,
            exit_status: Some(1),
        }
    }

    fn build(number: u64, failing: &[&str]) -> NightlyBuildSummary {
        NightlyBuildSummary {
            number,
            state: "failed".to_string(),
            branch: "develop".to_string(),
            commit: format!("commit{number}"),
            message: None,
            created_at: None,
            web_url: String::new(),
            failures: failing
                .iter()
                .map(|k| FailedJob::from(&job(k, "failed")))
                .collect(),
            job_count: 10,
            pull_requests: Vec::new(),
        }
    }

    #[test]
    fn retried_attempts_do_not_count_as_failures() {
        let mut retried = job("flaky", "failed");
        retried.retried = true;
        let jobs = vec![retried, job("real", "failed"), job("fine", "passed")];

        let failures = failures_of(&jobs);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].key, "real");
    }

    #[test]
    fn broken_and_waiting_failed_jobs_count_as_failures() {
        let jobs = vec![job("a", "broken"), job("b", "waiting_failed")];
        assert_eq!(failures_of(&jobs).len(), 2);
    }

    #[test]
    fn a_failure_absent_from_the_previous_build_is_new() {
        let builds = vec![build(1579, &["lmdb", "libp2p"]), build(1577, &["libp2p"])];
        let (classified, _) = classify(&builds);

        let lmdb = classified.iter().find(|f| f.job.key == "lmdb").unwrap();
        assert_eq!(lmdb.trend, Trend::New);
    }

    #[test]
    fn a_failure_present_in_earlier_builds_is_persistent_and_counted() {
        let builds = vec![
            build(1579, &["libp2p"]),
            build(1577, &["libp2p"]),
            build(1575, &["libp2p"]),
        ];
        let (classified, _) = classify(&builds);

        let libp2p = classified.iter().find(|f| f.job.key == "libp2p").unwrap();
        assert_eq!(libp2p.trend, Trend::Persistent { consecutive: 3 });
    }

    #[test]
    fn a_gap_restarts_the_consecutive_count() {
        // Failing now and three builds ago, but passing in between: the run
        // of consecutive failures is two, not three.
        let builds = vec![
            build(1579, &["libp2p"]),
            build(1577, &["libp2p"]),
            build(1575, &[]),
            build(1573, &["libp2p"]),
        ];
        let (classified, _) = classify(&builds);
        let libp2p = classified.iter().find(|f| f.job.key == "libp2p").unwrap();
        assert_eq!(libp2p.trend, Trend::Persistent { consecutive: 2 });
    }

    #[test]
    fn one_build_alone_yields_an_unknown_trend_not_a_new_one() {
        let builds = vec![build(1579, &["lmdb"])];
        let (classified, fixed) = classify(&builds);
        assert_eq!(classified[0].trend, Trend::Unknown);
        assert!(fixed.is_empty());
    }

    #[test]
    fn jobs_that_stopped_failing_are_listed_as_fixed() {
        let builds = vec![build(1579, &["libp2p"]), build(1577, &["libp2p", "lmdb"])];
        let (_, fixed) = classify(&builds);
        assert_eq!(fixed.len(), 1);
        assert_eq!(fixed[0].key, "lmdb");
    }

    #[test]
    fn no_builds_classify_to_nothing() {
        let (classified, fixed) = classify(&[]);
        assert!(classified.is_empty());
        assert!(fixed.is_empty());
    }

    #[test]
    fn new_and_persistent_views_partition_the_failures() {
        let builds = vec![build(1579, &["lmdb", "libp2p"]), build(1577, &["libp2p"])];
        let (latest_failures, fixed_since_previous) = classify(&builds);
        let report = NightlyReport {
            project: "mina".to_string(),
            pipeline: "nightly".to_string(),
            branch: Some("develop".to_string()),
            builds,
            latest_failures,
            fixed_since_previous,
            warnings: Vec::new(),
        };

        assert_eq!(report.new_failures().len(), 1);
        assert_eq!(report.persistent_failures().len(), 1);
        assert_eq!(report.new_failures()[0].job.key, "lmdb");
    }
}
