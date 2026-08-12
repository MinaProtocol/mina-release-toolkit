//! The join.
//!
//! Buildkite knows about builds, the Debian repositories know about packages
//! and the registries know about images. None of them knows about the others.
//! This module answers one question across all three: for this commit, what
//! exists where?

use std::collections::BTreeSet;

use futures::stream::{self, StreamExt};
use release_manager::artifacts::{
    archs_for_codename, artifact_has_docker, buckets_for_channel, expected_debian_packages,
    get_arch_suffix, get_docker_image_name, get_suffix, network_for_channel,
};

use crate::adapters::apt::{AptClient, ListingKey, Listings};
use crate::adapters::buildkite::{token_from_environment, BuildkiteClient};
use crate::adapters::docker::DockerClient;
use crate::adapters::github::GithubClient;
use crate::config::Project;
use crate::error::{OpsError, OpsResult};
use crate::git;
use crate::model::{BuildSummary, DebianEntry, DockerEntry, Inventory, Presence, VersionSource};

const MAX_CONCURRENT_LISTINGS: usize = 8;
/// Architecture of packages that are architecture-independent.
const ARCH_ALL: &str = "all";

#[derive(Debug, Clone)]
pub struct InventoryQuery {
    pub commit: Option<String>,
    pub version: Option<String>,
    pub channel: String,
    pub artifacts: Vec<String>,
    pub codenames: Vec<String>,
    pub network: Option<String>,
    pub profile: Option<String>,
    pub max_builds: usize,
    pub skip_buildkite: bool,
    pub skip_debian: bool,
    pub skip_docker: bool,
    pub skip_github: bool,
}

impl InventoryQuery {
    /// A query that asks only about builds. It reports no artifact coverage,
    /// so it neither resolves a version nor warns about the checks it skipped.
    pub fn is_builds_only(&self) -> bool {
        self.artifacts.is_empty() && self.skip_debian && self.skip_docker
    }
}

pub async fn collect(
    project_name: &str,
    project: &Project,
    query: &InventoryQuery,
) -> OpsResult<Inventory> {
    let mut warnings: Vec<String> = Vec::new();

    let network = query
        .network
        .clone()
        .unwrap_or_else(|| network_for_channel(&query.channel));
    let profile = query.profile.as_deref();

    let builds = collect_builds(project, query, &mut warnings).await;
    let pull_requests = collect_pull_requests(project, query, &mut warnings).await;

    let listings = if query.skip_debian {
        Listings::default()
    } else {
        collect_listings(project, query, &mut warnings).await
    };

    let builds_only = query.is_builds_only();

    let (version, version_source) = if builds_only {
        (None, None)
    } else {
        resolve_version(query, &builds, &listings, &mut warnings)
    };

    let debians = match &version {
        Some(v) => build_debian_entries(query, &listings, v, &network, profile),
        None => Vec::new(),
    };

    let dockers = match (&version, query.skip_docker) {
        (Some(v), false) => {
            build_docker_entries(project, query, v, &network, profile, &mut warnings).await
        }
        _ => Vec::new(),
    };

    if !builds_only {
        if query.skip_debian {
            warnings.push("Debian repositories were not checked (--skip-debian)".to_string());
        }
        if query.skip_docker {
            warnings.push("Docker registries were not checked (--skip-docker)".to_string());
        }
    }

    Ok(Inventory {
        project: project_name.to_string(),
        commit: query.commit.clone(),
        version,
        version_source,
        channel: query.channel.clone(),
        network,
        profile: query.profile.clone(),
        artifact_coverage_requested: !builds_only,
        pull_requests,
        builds,
        debians,
        dockers,
        warnings,
    })
}

async fn collect_builds(
    project: &Project,
    query: &InventoryQuery,
    warnings: &mut Vec<String>,
) -> Vec<BuildSummary> {
    let Some(commit) = query.commit.as_deref() else {
        return Vec::new();
    };
    if query.skip_buildkite {
        warnings.push("Buildkite was not queried (--skip-buildkite)".to_string());
        return Vec::new();
    }
    if !git::looks_like_full_sha(commit) {
        warnings.push(format!(
            "'{commit}' is not a full 40-character hash; Buildkite matches only full hashes. \
             Pass --repo-path to expand it, or pass the full hash."
        ));
        return Vec::new();
    }

    let token = match token_from_environment() {
        Ok(token) => token,
        Err(e) => {
            warnings.push(e.to_string());
            return Vec::new();
        }
    };

    let client = BuildkiteClient::new(&project.buildkite.org, token);
    let mut builds = match client.builds_for_commit(commit).await {
        Ok(builds) => builds,
        Err(e) => {
            warnings.push(format!("Buildkite query failed: {e}"));
            return Vec::new();
        }
    };

    builds.sort_by(|a, b| b.number.cmp(&a.number));

    // Artifact listing costs one request per build against a 200-per-minute
    // budget, so only the newest builds are enriched — and the limit is
    // reported rather than applied silently.
    let enrich_count = query.max_builds.min(builds.len());
    if builds.len() > enrich_count {
        warnings.push(format!(
            "{} builds found; artifacts listed for the newest {} only (--max-builds)",
            builds.len(),
            enrich_count
        ));
    }
    let artifact_warnings = client
        .enrich_with_artifacts(&mut builds[..enrich_count])
        .await;
    warnings.extend(artifact_warnings);

    builds
}

async fn collect_pull_requests(
    project: &Project,
    query: &InventoryQuery,
    warnings: &mut Vec<String>,
) -> Vec<crate::adapters::github::PullRequest> {
    let Some(commit) = query.commit.as_deref() else {
        return Vec::new();
    };
    if query.skip_github {
        return Vec::new();
    }

    let client = GithubClient::new();
    if let Err(reason) = client.available().await {
        warnings.push(format!("pull requests not resolved: {reason}"));
        return Vec::new();
    }

    match client.pulls_for_commit(&project.repo, commit).await {
        Ok(pulls) => pulls,
        Err(e) => {
            warnings.push(e.to_string());
            Vec::new()
        }
    }
}

/// Every (bucket, component, codename, arch) listing the query needs.
fn listing_keys(project_channel: &str, codenames: &[String]) -> Vec<ListingKey> {
    let mut keys = Vec::new();
    for bucket in buckets_for_channel(project_channel) {
        for codename in codenames {
            let mut archs: Vec<String> = archs_for_codename(codename)
                .iter()
                .map(|a| a.to_string())
                .collect();
            // Architecture-independent packages such as mina-<network>-config
            // live under their own index.
            archs.push(ARCH_ALL.to_string());
            for arch in archs {
                keys.push(ListingKey {
                    bucket: bucket.clone(),
                    component: project_channel.to_string(),
                    codename: codename.clone(),
                    arch,
                });
            }
        }
    }
    keys
}

async fn collect_listings(
    project: &Project,
    query: &InventoryQuery,
    warnings: &mut Vec<String>,
) -> Listings {
    let client = AptClient::new(&project.apt.region);
    let keys = listing_keys(&query.channel, &query.codenames);

    let results = stream::iter(keys.into_iter().map(|key| {
        let client = &client;
        async move {
            let outcome = client.list(&key).await;
            (key, outcome)
        }
    }))
    .buffer_unordered(MAX_CONCURRENT_LISTINGS)
    .collect::<Vec<_>>()
    .await;

    let mut listings = Listings::default();
    for (key, outcome) in results {
        match outcome {
            Ok(listing) => {
                listings.ok.insert(key, listing);
            }
            Err(e) => {
                warnings.push(e.to_string());
                listings.failed.insert(key, e.to_string());
            }
        }
    }
    listings
}

/// Explicit version, else the build artifacts, else the repositories.
///
/// The last path matters most in practice: Buildkite purges artifacts, and the
/// commit's short hash stays in the package version forever.
fn resolve_version(
    query: &InventoryQuery,
    builds: &[BuildSummary],
    listings: &Listings,
    warnings: &mut Vec<String>,
) -> (Option<String>, Option<VersionSource>) {
    if let Some(version) = &query.version {
        return (Some(version.clone()), Some(VersionSource::Given));
    }

    let from_builds: Vec<String> = builds
        .iter()
        .flat_map(|b| b.versions.iter().cloned())
        .collect();
    if let Some(version) = first_unique(&from_builds, "Buildkite artifacts", warnings) {
        return (Some(version), Some(VersionSource::BuildkiteArtifacts));
    }

    if let Some(commit) = &query.commit {
        let short = git::short_hash(commit);
        let from_repos: Vec<String> = listings
            .ok
            .values()
            .flat_map(|l| l.versions_for_commit(&short))
            .collect();
        if let Some(version) = first_unique(&from_repos, "the Debian repositories", warnings) {
            if !builds.is_empty() {
                warnings.push(
                    "version recovered from the Debian repositories. Mina pipelines do not \
                     upload .deb files to Buildkite, so this is the normal path."
                        .to_string(),
                );
            }
            return (Some(version), Some(VersionSource::DebianRepository));
        }
    }

    warnings.push(
        "no version could be resolved for this commit. Pass --version to check a known version."
            .to_string(),
    );
    (None, None)
}

/// Picks one value, warning when the candidates disagree instead of choosing
/// silently.
fn first_unique(candidates: &[String], source: &str, warnings: &mut Vec<String>) -> Option<String> {
    let distinct: BTreeSet<&String> = candidates.iter().collect();
    match distinct.len() {
        0 => None,
        1 => Some(candidates[0].clone()),
        _ => {
            let all: Vec<&str> = distinct.iter().map(|s| s.as_str()).collect();
            warnings.push(format!(
                "{source} report more than one version for this commit: {}. Using {}.",
                all.join(", "),
                candidates[0]
            ));
            Some(candidates[0].clone())
        }
    }
}

fn build_debian_entries(
    query: &InventoryQuery,
    listings: &Listings,
    version: &str,
    network: &str,
    profile: Option<&str>,
) -> Vec<DebianEntry> {
    let mut entries = Vec::new();

    for bucket in buckets_for_channel(&query.channel) {
        for codename in &query.codenames {
            for arch in archs_for_codename(codename) {
                for artifact in &query.artifacts {
                    for expected in expected_debian_packages(artifact, arch, network, profile) {
                        let key = ListingKey {
                            bucket: bucket.clone(),
                            component: query.channel.clone(),
                            codename: codename.clone(),
                            arch: expected.arch.clone(),
                        };
                        let presence = match listings.ok.get(&key) {
                            Some(listing) => {
                                if listing.contains(&expected.package, version, &expected.arch) {
                                    Presence::Present
                                } else {
                                    Presence::Missing
                                }
                            }
                            None => Presence::Unknown(
                                listings
                                    .failed
                                    .get(&key)
                                    .cloned()
                                    .unwrap_or_else(|| "repository not listed".to_string()),
                            ),
                        };
                        entries.push(DebianEntry {
                            package: expected.package,
                            version: version.to_string(),
                            codename: codename.clone(),
                            arch: expected.arch,
                            component: query.channel.clone(),
                            bucket: bucket.clone(),
                            presence,
                        });
                    }
                }
            }
        }
    }

    entries.sort_by(|a, b| {
        (&a.bucket, &a.codename, &a.package, &a.arch).cmp(&(
            &b.bucket,
            &b.codename,
            &b.package,
            &b.arch,
        ))
    });
    entries.dedup_by(|a, b| {
        a.bucket == b.bucket
            && a.codename == b.codename
            && a.package == b.package
            && a.arch == b.arch
    });
    entries
}

/// Docker references a commit's build is expected to have pushed.
pub fn expected_docker_entries(
    repo: &str,
    artifacts: &[String],
    codenames: &[String],
    version: &str,
    network: &str,
    profile: Option<&str>,
) -> Vec<DockerEntry> {
    let mut entries: Vec<DockerEntry> = Vec::new();
    for artifact in artifacts {
        if !artifact_has_docker(artifact) {
            continue;
        }
        for codename in codenames {
            for arch in archs_for_codename(codename) {
                let tag = format!(
                    "{}-{}{}{}",
                    version,
                    codename,
                    get_suffix(artifact, Some(network), profile),
                    get_arch_suffix(arch)
                );
                let image = get_docker_image_name(artifact).to_string();
                if entries.iter().any(|e| e.image == image && e.tag == tag) {
                    continue;
                }
                entries.push(DockerEntry {
                    image,
                    tag,
                    repo: repo.to_string(),
                    presence: Presence::Unknown("not checked".to_string()),
                });
            }
        }
    }
    entries
}

async fn build_docker_entries(
    project: &Project,
    query: &InventoryQuery,
    version: &str,
    network: &str,
    profile: Option<&str>,
    warnings: &mut Vec<String>,
) -> Vec<DockerEntry> {
    let repo = if query.channel == "stable" {
        &project.docker.docker_io
    } else {
        &project.docker.gcr
    };

    let mut entries = expected_docker_entries(
        repo,
        &query.artifacts,
        &query.codenames,
        version,
        network,
        profile,
    );

    let client = DockerClient::new();
    if !client.available().await {
        let reason = "docker is not on PATH, so image presence is unknown".to_string();
        warnings.push(reason.clone());
        for entry in &mut entries {
            entry.presence = Presence::Unknown(reason.clone());
        }
        return entries;
    }

    let references: Vec<String> = entries
        .iter()
        .map(|e| format!("{}/{}:{}", e.repo, e.image, e.tag))
        .collect();
    let presences = client.manifest_presence_many(&references).await;
    for (entry, presence) in entries.iter_mut().zip(presences) {
        entry.presence = presence;
    }
    entries
}

/// Resolves the commit a query should use, expanding an abbreviated hash with
/// a local checkout when one was given.
pub fn resolve_commit(commit: &str, repo_path: Option<&std::path::Path>) -> OpsResult<String> {
    if git::looks_like_full_sha(commit) {
        return Ok(commit.to_string());
    }
    match repo_path.and_then(|p| git::expand_commit(p, commit)) {
        Some(full) => Ok(full),
        None if repo_path.is_some() => Err(OpsError::Other(format!(
            "'{commit}' is not a commit in the given checkout"
        ))),
        None => Ok(commit.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::apt::parse_listing;

    fn query() -> InventoryQuery {
        InventoryQuery {
            commit: Some("143b9cc0ae1d33bd16d0d1623f8b636d49646b11".to_string()),
            version: None,
            channel: "alpha".to_string(),
            artifacts: vec!["mina-daemon".to_string(), "mina-logproc".to_string()],
            codenames: vec!["noble".to_string()],
            network: None,
            profile: None,
            max_builds: 5,
            skip_buildkite: false,
            skip_debian: false,
            skip_docker: false,
            skip_github: true,
        }
    }

    #[test]
    fn listing_keys_include_the_arch_independent_index() {
        let keys = listing_keys("alpha", &["noble".to_string()]);
        let archs: BTreeSet<&str> = keys.iter().map(|k| k.arch.as_str()).collect();
        assert!(archs.contains("amd64"));
        assert!(archs.contains("arm64"));
        assert!(archs.contains("all"));
    }

    #[test]
    fn listing_keys_cover_every_bucket_of_the_channel() {
        let keys = listing_keys("alpha", &["noble".to_string()]);
        let buckets: BTreeSet<&str> = keys.iter().map(|k| k.bucket.as_str()).collect();
        assert_eq!(buckets.len(), buckets_for_channel("alpha").len());
    }

    #[test]
    fn given_version_wins_over_every_other_source() {
        let mut q = query();
        q.version = Some("9.9.9-deadbee".to_string());
        let mut warnings = Vec::new();
        let (version, source) = resolve_version(&q, &[], &Listings::default(), &mut warnings);
        assert_eq!(version.unwrap(), "9.9.9-deadbee");
        assert_eq!(source.unwrap(), VersionSource::Given);
    }

    #[test]
    fn version_falls_back_to_the_repositories_when_artifacts_are_purged() {
        let q = query();
        let mut listings = Listings::default();
        listings.ok.insert(
            ListingKey {
                bucket: "packages.o1test.net".into(),
                component: "alpha".into(),
                codename: "noble".into(),
                arch: "amd64".into(),
            },
            parse_listing("mina-devnet  3.2.0-alpha1-143b9cc  amd64\n"),
        );

        let mut warnings = Vec::new();
        let (version, source) = resolve_version(&q, &[], &listings, &mut warnings);
        assert_eq!(version.unwrap(), "3.2.0-alpha1-143b9cc");
        assert_eq!(source.unwrap(), VersionSource::DebianRepository);
    }

    #[test]
    fn unresolved_version_is_reported_not_guessed() {
        let q = query();
        let mut warnings = Vec::new();
        let (version, source) = resolve_version(&q, &[], &Listings::default(), &mut warnings);
        assert!(version.is_none());
        assert!(source.is_none());
        assert!(warnings.iter().any(|w| w.contains("no version")));
    }

    #[test]
    fn disagreeing_versions_produce_a_warning() {
        let mut warnings = Vec::new();
        let chosen = first_unique(
            &["3.2.0-aaaaaaa".to_string(), "3.2.0-bbbbbbb".to_string()],
            "Buildkite artifacts",
            &mut warnings,
        );
        assert_eq!(chosen.unwrap(), "3.2.0-aaaaaaa");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("more than one version"));
    }

    #[test]
    fn a_failed_listing_yields_unknown_not_missing() {
        let q = query();
        let mut listings = Listings::default();
        for key in listing_keys(&q.channel, &q.codenames) {
            listings.failed.insert(key, "deb-s3 exploded".to_string());
        }

        let entries = build_debian_entries(&q, &listings, "3.2.0-143b9cc", "devnet", None);
        assert!(!entries.is_empty());
        assert!(
            entries.iter().all(|e| e.presence.is_unknown()),
            "an unreadable repository must not be reported as missing"
        );
    }

    #[test]
    fn present_and_missing_are_distinguished() {
        let q = query();
        let mut listings = Listings::default();
        for key in listing_keys(&q.channel, &q.codenames) {
            let text = if key.arch == "amd64" {
                "mina-devnet  3.2.0-143b9cc  amd64\n"
            } else {
                ""
            };
            listings.ok.insert(key, parse_listing(text));
        }

        let entries = build_debian_entries(&q, &listings, "3.2.0-143b9cc", "devnet", None);
        let daemon_amd64: Vec<_> = entries
            .iter()
            .filter(|e| e.package == "mina-devnet" && e.arch == "amd64")
            .collect();
        assert!(!daemon_amd64.is_empty());
        assert!(daemon_amd64.iter().all(|e| e.presence.is_present()));

        let logproc: Vec<_> = entries
            .iter()
            .filter(|e| e.package == "mina-logproc")
            .collect();
        assert!(!logproc.is_empty());
        assert!(logproc.iter().all(|e| e.presence == Presence::Missing));
    }

    #[test]
    fn docker_entries_skip_debian_only_artifacts() {
        let entries = expected_docker_entries(
            "gcr.io/o1labs-192920",
            &["mina-daemon".to_string(), "mina-logproc".to_string()],
            &["noble".to_string()],
            "3.2.0-143b9cc",
            "devnet",
            None,
        );
        assert!(entries.iter().all(|e| e.image != "mina-logproc"));
        assert!(entries.iter().any(|e| e.image == "mina-daemon"));
    }

    #[test]
    fn docker_tags_carry_codename_network_and_arch() {
        let entries = expected_docker_entries(
            "gcr.io/o1labs-192920",
            &["mina-daemon".to_string()],
            &["noble".to_string()],
            "3.2.0-143b9cc",
            "devnet",
            None,
        );
        let tags: Vec<&str> = entries.iter().map(|e| e.tag.as_str()).collect();
        assert!(tags.contains(&"3.2.0-143b9cc-noble-devnet"));
        assert!(tags.contains(&"3.2.0-143b9cc-noble-devnet-arm64"));
    }

    #[test]
    fn full_hashes_pass_through_commit_resolution() {
        let full = "143b9cc0ae1d33bd16d0d1623f8b636d49646b11";
        assert_eq!(resolve_commit(full, None).unwrap(), full);
    }

    #[test]
    fn short_hash_without_a_checkout_is_left_alone() {
        assert_eq!(resolve_commit("143b9cc", None).unwrap(), "143b9cc");
    }
}
