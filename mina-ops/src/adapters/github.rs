//! GitHub adapter.
//!
//! Calls `gh api`, so the tool inherits whatever authentication the operator
//! already has and holds no GitHub token of its own. Answers one question:
//! which pull request does this commit belong to — that is, who owns the
//! change that broke the build.

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{OpsError, OpsResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    /// `open`, `closed`, or `merged` — merged is reported in place of closed,
    /// because for triage the two mean different things.
    pub state: String,
    pub author: Option<String>,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct ApiPullRequest {
    number: u64,
    title: String,
    state: String,
    merged_at: Option<String>,
    html_url: String,
    user: Option<ApiUser>,
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    login: String,
}

impl From<ApiPullRequest> for PullRequest {
    fn from(pr: ApiPullRequest) -> Self {
        let state = if pr.merged_at.is_some() {
            "merged".to_string()
        } else {
            pr.state
        };
        PullRequest {
            number: pr.number,
            title: pr.title.trim().to_string(),
            state,
            author: pr.user.map(|u| u.login),
            url: pr.html_url,
        }
    }
}

pub struct GithubClient;

impl GithubClient {
    pub fn new() -> Self {
        Self
    }

    /// Whether `gh` is installed and authenticated. Checked once so a machine
    /// without it reports a single clear reason.
    pub async fn available(&self) -> Result<(), String> {
        let output = Command::new("gh")
            .args(["auth", "status"])
            .output()
            .await
            .map_err(|e| format!("gh is not on PATH: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err("gh is installed but not authenticated; run `gh auth login`".to_string())
        }
    }

    /// Pull requests that contain a commit.
    ///
    /// A commit on a branch that was never opened as a pull request yields an
    /// empty list, which is an answer rather than an error.
    pub async fn pulls_for_commit(&self, repo: &str, commit: &str) -> OpsResult<Vec<PullRequest>> {
        let output = Command::new("gh")
            .args([
                "api",
                &format!("repos/{repo}/commits/{commit}/pulls"),
                "--header",
                "Accept: application/vnd.github+json",
            ])
            .output()
            .await
            .map_err(|e| OpsError::Other(format!("could not run gh: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(OpsError::Other(format!(
                "gh api failed for {repo}@{}: {}",
                short(commit),
                first_line(&stderr)
            )));
        }

        let pulls: Vec<ApiPullRequest> = serde_json::from_slice(&output.stdout)
            .map_err(|e| OpsError::Other(format!("cannot parse the gh response: {e}")))?;
        Ok(pulls.into_iter().map(Into::into).collect())
    }
}

impl Default for GithubClient {
    fn default() -> Self {
        Self::new()
    }
}

fn short(commit: &str) -> String {
    commit.chars().take(9).collect()
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

    fn api_pr(state: &str, merged_at: Option<&str>) -> ApiPullRequest {
        ApiPullRequest {
            number: 19188,
            title: "  fix the nightly  ".to_string(),
            state: state.to_string(),
            merged_at: merged_at.map(str::to_string),
            html_url: "https://github.com/MinaProtocol/mina/pull/19188".to_string(),
            user: Some(ApiUser {
                login: "dkijania".to_string(),
            }),
        }
    }

    #[test]
    fn a_merged_pull_request_is_reported_as_merged_not_closed() {
        let pr: PullRequest = api_pr("closed", Some("2026-08-01T00:00:00Z")).into();
        assert_eq!(pr.state, "merged");
    }

    #[test]
    fn a_closed_but_unmerged_pull_request_stays_closed() {
        let pr: PullRequest = api_pr("closed", None).into();
        assert_eq!(pr.state, "closed");
    }

    #[test]
    fn titles_are_trimmed_and_the_author_is_kept() {
        let pr: PullRequest = api_pr("open", None).into();
        assert_eq!(pr.title, "fix the nightly");
        assert_eq!(pr.author.as_deref(), Some("dkijania"));
        assert_eq!(pr.number, 19188);
    }

    #[test]
    fn errors_report_only_their_first_line() {
        assert_eq!(
            first_line("\ngh: Not Found (HTTP 404)\ntrace"),
            "gh: Not Found (HTTP 404)"
        );
    }
}
