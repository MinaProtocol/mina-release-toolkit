//! Debian repository adapter.
//!
//! Wraps `deb-s3 list`, the same tool `release-manager` uses, so both agree on
//! what a repository contains. One listing is fetched per
//! (bucket, component, codename, architecture) and then queried in memory.

use std::collections::HashMap;

use tokio::process::Command;

use crate::error::OpsResult;

/// One row of `deb-s3 list`: `name  version  arch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebRow {
    pub package: String,
    pub version: String,
    pub arch: String,
}

/// The listing of one (bucket, component, codename, arch) combination.
#[derive(Debug, Clone)]
pub struct Listing {
    pub rows: Vec<DebRow>,
}

impl Listing {
    pub fn contains(&self, package: &str, version: &str, arch: &str) -> bool {
        self.rows
            .iter()
            .any(|r| r.package == package && r.version == version && r.arch == arch)
    }

    /// Versions of any package whose version ends with `-<short_commit>`.
    ///
    /// Mina package versions embed the short commit hash, for example
    /// `3.2.0-49b523c`. This is what lets a commit be resolved to a version
    /// even after Buildkite has purged the build's artifacts.
    pub fn versions_for_commit(&self, short_commit: &str) -> Vec<String> {
        let suffix = format!("-{short_commit}");
        let mut out: Vec<String> = Vec::new();
        for row in &self.rows {
            if row.version.ends_with(&suffix) && !out.contains(&row.version) {
                out.push(row.version.clone());
            }
        }
        out
    }
}

pub fn parse_listing(text: &str) -> Listing {
    let rows = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let package = fields.next()?;
            let version = fields.next()?;
            let arch = fields.next()?;
            Some(DebRow {
                package: package.to_string(),
                version: version.to_string(),
                arch: arch.to_string(),
            })
        })
        .collect();
    Listing { rows }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListingKey {
    pub bucket: String,
    pub component: String,
    pub codename: String,
    pub arch: String,
}

pub struct AptClient {
    region: String,
}

impl AptClient {
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            region: region.into(),
        }
    }

    /// `deb-s3 list` for one combination.
    ///
    /// An empty repository and a failed call are different outcomes, so a
    /// non-zero exit is an error rather than an empty listing. Reporting a
    /// failed query as "no packages" would read as "artifacts missing".
    pub async fn list(&self, key: &ListingKey) -> OpsResult<Listing> {
        let output = Command::new("deb-s3")
            .args([
                "list",
                &format!("--bucket={}", key.bucket),
                &format!("--s3-region={}", self.region),
                "--component",
                &key.component,
                "--codename",
                &key.codename,
                "--arch",
                &key.arch,
            ])
            .output()
            .await
            .map_err(|e| {
                crate::error::OpsError::Other(format!("could not run deb-s3 (is it on PATH?): {e}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::OpsError::Other(format!(
                "deb-s3 list failed for {}/{}/{}/{}: {}",
                key.bucket,
                key.component,
                key.codename,
                key.arch,
                stderr.trim()
            )));
        }

        Ok(parse_listing(&String::from_utf8_lossy(&output.stdout)))
    }
}

/// Listings collected so far, plus the failures, so a caller can report
/// `Unknown` instead of `Missing` for the combinations that could not be read.
#[derive(Debug, Default)]
pub struct Listings {
    pub ok: HashMap<ListingKey, Listing>,
    pub failed: HashMap<ListingKey, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "mina-logproc          3.2.0-49b523c  amd64\n\
                          mina-mainnet          3.2.0-49b523c  amd64\n\
                          mina-archive-mainnet  3.2.0-49b523c  amd64\n\
                          mina-devnet           3.2.0-alpha1-7f94ae0  amd64\n";

    #[test]
    fn parses_real_deb_s3_output() {
        let listing = parse_listing(SAMPLE);
        assert_eq!(listing.rows.len(), 4);
        assert_eq!(listing.rows[0].package, "mina-logproc");
        assert_eq!(listing.rows[0].version, "3.2.0-49b523c");
        assert_eq!(listing.rows[0].arch, "amd64");
    }

    #[test]
    fn ignores_short_lines() {
        let listing = parse_listing("garbage\nmina-logproc 1.0 amd64\n\n");
        assert_eq!(listing.rows.len(), 1);
    }

    #[test]
    fn contains_matches_all_three_fields() {
        let listing = parse_listing(SAMPLE);
        assert!(listing.contains("mina-mainnet", "3.2.0-49b523c", "amd64"));
        assert!(!listing.contains("mina-mainnet", "3.2.0-49b523c", "arm64"));
        assert!(!listing.contains("mina-mainnet", "9.9.9", "amd64"));
    }

    #[test]
    fn resolves_versions_from_a_short_commit() {
        let listing = parse_listing(SAMPLE);
        assert_eq!(
            listing.versions_for_commit("49b523c"),
            vec!["3.2.0-49b523c"]
        );
        assert_eq!(
            listing.versions_for_commit("7f94ae0"),
            vec!["3.2.0-alpha1-7f94ae0"]
        );
        assert!(listing.versions_for_commit("deadbee").is_empty());
    }

    #[test]
    fn commit_match_is_anchored_to_the_end_of_the_version() {
        // "523c" is a substring of the version but not the commit field.
        let listing = parse_listing(SAMPLE);
        assert!(listing.versions_for_commit("523c").is_empty());
    }
}
