//! Docker registry adapter.
//!
//! Uses `docker manifest inspect`, as `release-manager` does, so no registry
//! credentials are handled by this tool. The distinction that matters is
//! between "the registry says there is no such tag" and "the check could not
//! run": an authentication failure must never be reported as a missing image.

use futures::stream::{self, StreamExt};
use tokio::process::Command;

use crate::model::Presence;

const MAX_CONCURRENT_CHECKS: usize = 8;

pub struct DockerClient;

impl DockerClient {
    pub fn new() -> Self {
        Self
    }

    /// Whether the docker CLI is usable at all. Checked once, so a machine
    /// without docker reports one clear reason instead of one per image.
    pub async fn available(&self) -> bool {
        Command::new("docker")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    pub async fn manifest_presence(&self, reference: &str) -> Presence {
        let output = Command::new("docker")
            .args(["manifest", "inspect", reference])
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => Presence::Present,
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
                if is_absent(&stderr) {
                    Presence::Missing
                } else {
                    Presence::Unknown(summarise(&String::from_utf8_lossy(&out.stderr)))
                }
            }
            Err(e) => Presence::Unknown(format!("could not run docker: {e}")),
        }
    }

    pub async fn manifest_presence_many(&self, references: &[String]) -> Vec<Presence> {
        // Owned references, for the same lifetime reason as above.
        let owned: Vec<String> = references.to_vec();
        stream::iter(
            owned
                .into_iter()
                .map(|reference| async move { self.manifest_presence(&reference).await }),
        )
        .buffered(MAX_CONCURRENT_CHECKS)
        .collect()
        .await
    }
}

impl Default for DockerClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Registry messages that genuinely mean "this tag does not exist". Anything
/// else — denied, unauthorized, timeouts — is an unknown, not a miss.
fn is_absent(stderr_lowercase: &str) -> bool {
    stderr_lowercase.contains("manifest unknown")
        || stderr_lowercase.contains("no such manifest")
        || stderr_lowercase.contains("not found")
}

fn summarise(stderr: &str) -> String {
    let line = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("docker manifest inspect failed")
        .trim();
    let max = 160;
    if line.len() > max {
        format!("{}…", &line[..max])
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_messages_are_recognised() {
        assert!(is_absent("manifest unknown"));
        assert!(is_absent("error: no such manifest: gcr.io/x:y"));
        assert!(is_absent("manifest for gcr.io/x:y not found"));
    }

    #[test]
    fn authentication_failures_are_not_absence() {
        assert!(!is_absent("denied: permission denied"));
        assert!(!is_absent("unauthorized: authentication required"));
        assert!(!is_absent("net/http: tls handshake timeout"));
    }

    #[test]
    fn summary_takes_the_first_useful_line() {
        assert_eq!(summarise("\n\ndenied: nope\nsecond line"), "denied: nope");
    }

    #[test]
    fn summary_is_truncated() {
        let long = "x".repeat(300);
        let out = summarise(&long);
        assert!(out.chars().count() <= 161);
        assert!(out.ends_with('…'));
    }
}
