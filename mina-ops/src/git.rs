//! Local git helpers.
//!
//! Buildkite matches full 40-character commit hashes, while Mina package
//! versions embed the 7-character short hash. Both forms are needed, so a
//! local checkout is used to convert between them when one is available.

use std::path::Path;
use std::process::Command;

/// Mina's `export-git-env-vars.sh` uses 7 characters in package versions.
pub const SHORT_HASH_LENGTH: usize = 7;

pub fn looks_like_full_sha(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn short_hash(commit: &str) -> String {
    commit.chars().take(SHORT_HASH_LENGTH).collect()
}

/// Expands an abbreviated commit using a local checkout.
///
/// Returns `None` when there is no checkout, the path is not a repository, or
/// the revision is unknown there.
pub fn expand_commit(repo_path: &Path, commit: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("rev-parse")
        .arg(format!("{commit}^{{commit}}"))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
    looks_like_full_sha(&resolved).then_some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_sha_recognition() {
        assert!(looks_like_full_sha(
            "143b9cc0ae1d33bd16d0d1623f8b636d49646b11"
        ));
        assert!(!looks_like_full_sha("143b9cc"));
        assert!(!looks_like_full_sha(
            "zzzb9cc0ae1d33bd16d0d1623f8b636d49646b11"
        ));
    }

    #[test]
    fn short_hash_is_seven_characters() {
        assert_eq!(
            short_hash("143b9cc0ae1d33bd16d0d1623f8b636d49646b11"),
            "143b9cc"
        );
        // Already short: returned unchanged rather than padded.
        assert_eq!(short_hash("143b9cc"), "143b9cc");
        assert_eq!(short_hash("abc"), "abc");
    }

    #[test]
    fn expanding_in_a_non_repository_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(expand_commit(dir.path(), "HEAD"), None);
    }
}
