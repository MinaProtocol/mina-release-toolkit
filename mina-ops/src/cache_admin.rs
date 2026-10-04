//! The cache tab's logic: what is in the cache, and what may be removed.
//!
//! Deletion here is the only destructive operation in `mina-ops`, and it acts
//! on the shared CI cache that Buildkite reads from — a hardfork run points
//! straight at a build folder through `USE_ARTIFACTS_FROM_BUILDKITE_BUILD`.
//! Every guard below exists because of that, and each one is checked on the
//! server rather than in the browser.

use serde::{Deserialize, Serialize};

use crate::adapters::buildkite::{token_from_environment, BuildkiteClient};
use crate::adapters::hetzner::{self, CacheClient, CacheEntry, CachedDeb, CachedImage};
use crate::config::Project;
use crate::error::{OpsError, OpsResult};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheListing {
    /// Where the listing came from, so a surprising answer can be traced.
    pub source: String,
    pub entries: Vec<CacheEntry>,
    pub total_size: Option<String>,
    pub build_count: usize,
    /// Entries that are not build folders — `legacy`, `docker-cache` and the
    /// like. Reported so they are visible, never deletable.
    pub shared_count: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheDetail {
    pub build_id: String,
    pub source: String,
    pub debians: Vec<CachedDeb>,
    /// Image tarballs, for `docker-cache`. A folder holds packages or images,
    /// not both, so one of the two lists is always empty.
    #[serde(default)]
    pub images: Vec<CachedImage>,
    /// Distinct versions across the packages, which is usually the one thing
    /// somebody wants from a build folder.
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeleteRequest {
    pub build_id: String,
    /// Must equal `build_id`. Typing it again is what separates a deletion
    /// from a mis-click.
    #[serde(default)]
    pub confirm_build_id: String,
    /// When true, nothing is removed and the report says what would be.
    #[serde(default = "default_true")]
    pub dry_run: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteReport {
    pub build_id: String,
    pub dry_run: bool,
    /// True only when a deletion actually happened.
    pub deleted: bool,
    pub freed: Option<String>,
    /// Every guard that was evaluated, in the order it ran.
    pub guards: Vec<Guard>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Guard {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

impl Guard {
    fn pass(name: &str, detail: impl Into<String>) -> Self {
        Guard {
            name: name.to_string(),
            passed: true,
            detail: detail.into(),
        }
    }
    fn block(name: &str, detail: impl Into<String>) -> Self {
        Guard {
            name: name.to_string(),
            passed: false,
            detail: detail.into(),
        }
    }
}

/// The shared folder that holds docker images rather than packages.
pub const DOCKER_CACHE: &str = "docker-cache";

fn client(project: &Project) -> OpsResult<CacheClient> {
    hetzner::resolve_route(project.cache.as_ref())
        .map(CacheClient::new)
        .ok_or_else(|| {
            OpsError::Config(
                "no CI cache configured. Set cache.root or cache.ssh in your projects.yaml, or \
                 MINA_OPS_CACHE_ROOT / MINA_OPS_CACHE_SSH_*"
                    .to_string(),
            )
        })
}

pub async fn list(project: &Project) -> OpsResult<CacheListing> {
    let client = client(project)?;
    let source = client.route().describe();
    let entries = client.list_entries().await?;

    let build_count = entries.iter().filter(|e| e.is_build).count();
    let shared_count = entries.len() - build_count;

    let mut warnings = Vec::new();
    if entries.iter().all(|e| e.size.is_none()) {
        warnings.push(
            "sizes are not shown for a mounted cache: measuring them would walk the whole tree"
                .to_string(),
        );
    }

    let total_bytes: u64 = entries.iter().filter_map(|e| e.size_bytes).sum();
    let total_size = (total_bytes > 0).then(|| bytes_to_human(total_bytes));

    Ok(CacheListing {
        source,
        entries,
        total_size,
        build_count,
        shared_count,
        warnings,
    })
}

/// Packages under any cache entry — a build folder, or a shared one such as
/// `legacy`, which is the folder people most often need to look inside.
pub async fn detail(project: &Project, entry: &str) -> OpsResult<CacheDetail> {
    if !CacheClient::is_safe_entry_name(entry) {
        return Err(OpsError::Other(format!(
            "'{entry}' is not a valid cache entry name"
        )));
    }
    let client = client(project)?;
    let source = client.route().describe();

    // docker-cache holds image tarballs rather than packages, so it is read
    // with the reader that understands them.
    if entry == DOCKER_CACHE {
        let images = client.images_for_entry(entry).await?;
        let mut versions: Vec<String> = images.iter().filter_map(|i| i.commit.clone()).collect();
        versions.sort();
        versions.dedup();
        return Ok(CacheDetail {
            build_id: entry.to_string(),
            source,
            debians: Vec::new(),
            images,
            versions,
        });
    }

    let (_, debians) = client.debians_for_entry(entry).await;

    let mut versions: Vec<String> = debians.iter().filter_map(|d| d.version.clone()).collect();
    versions.sort();
    versions.dedup();

    Ok(CacheDetail {
        build_id: entry.to_string(),
        source,
        debians,
        images: Vec::new(),
        versions,
    })
}

/// Runs every guard, then deletes only if all of them passed and this is not
/// a dry run.
pub async fn delete(project: &Project, request: &DeleteRequest) -> OpsResult<DeleteReport> {
    let mut guards = Vec::new();

    // 1. Only build folders. `legacy` holds restored packages and
    // `docker-cache` is shared; neither is a build and neither is removable.
    if hetzner::looks_like_build_id(&request.build_id) {
        guards.push(Guard::pass("is a build folder", &request.build_id));
    } else {
        guards.push(Guard::block(
            "is a build folder",
            "only build UUID folders can be removed; shared folders such as legacy and \
             docker-cache are never touched",
        ));
    }

    // 2. The identifier typed back must match.
    if request.confirm_build_id == request.build_id {
        guards.push(Guard::pass(
            "confirmation matches",
            "the UUID was typed back",
        ));
    } else {
        guards.push(Guard::block(
            "confirmation matches",
            "type the build UUID again to confirm",
        ));
    }

    // 3. Buildkite must not be using it right now.
    guards.push(check_not_in_flight(project, &request.build_id).await);

    let client = client(project)?;
    let listing = client.list_entries().await.unwrap_or_default();
    let entry = listing.iter().find(|e| e.name == request.build_id);
    let freed = entry.and_then(|e| e.size.clone());

    // 4. It has to exist.
    match entry {
        Some(_) => guards.push(Guard::pass(
            "exists in the cache",
            freed.clone().unwrap_or_else(|| "present".to_string()),
        )),
        None => guards.push(Guard::block(
            "exists in the cache",
            "no such build folder — it may already have been removed",
        )),
    }

    let blocked = guards.iter().any(|g| !g.passed);
    if blocked {
        return Ok(DeleteReport {
            build_id: request.build_id.clone(),
            dry_run: request.dry_run,
            deleted: false,
            freed,
            guards,
            message: "refused: not every guard passed".to_string(),
        });
    }

    if request.dry_run {
        return Ok(DeleteReport {
            build_id: request.build_id.clone(),
            dry_run: true,
            deleted: false,
            freed: freed.clone(),
            guards,
            message: format!(
                "would remove {} and free {}",
                request.build_id,
                freed.unwrap_or_else(|| "an unknown amount".to_string())
            ),
        });
    }

    client.delete_build(&request.build_id).await?;
    Ok(DeleteReport {
        build_id: request.build_id.clone(),
        dry_run: false,
        deleted: true,
        freed: freed.clone(),
        guards,
        message: format!(
            "removed {} and freed {}",
            request.build_id,
            freed.unwrap_or_else(|| "an unknown amount".to_string())
        ),
    })
}

/// Refuses a build Buildkite is running or has scheduled.
///
/// If the state cannot be read the guard blocks rather than allows: not
/// knowing whether CI is using a folder is not a reason to delete it.
async fn check_not_in_flight(project: &Project, build_id: &str) -> Guard {
    let name = "not in use by Buildkite";
    let token = match token_from_environment() {
        Ok(token) => token,
        Err(e) => return Guard::block(name, format!("could not check: {e}")),
    };

    let client = BuildkiteClient::new(&project.buildkite.org, token);
    match client.builds_in_flight().await {
        Ok(in_flight) => {
            if in_flight.iter().any(|b| b.id == build_id) {
                Guard::block(name, "Buildkite is running or has scheduled this build")
            } else {
                Guard::pass(
                    name,
                    format!(
                        "{} builds are in flight, none of them this one",
                        in_flight.len()
                    ),
                )
            }
        }
        Err(e) => Guard::block(name, format!("could not check: {e}")),
    }
}

pub fn bytes_to_human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(build: &str, confirm: &str, dry_run: bool) -> DeleteRequest {
        DeleteRequest {
            build_id: build.to_string(),
            confirm_build_id: confirm.to_string(),
            dry_run,
        }
    }

    fn guards_of(request: &DeleteRequest) -> Vec<Guard> {
        // The two guards that need nothing external, in isolation.
        let mut guards = Vec::new();
        if hetzner::looks_like_build_id(&request.build_id) {
            guards.push(Guard::pass("is a build folder", ""));
        } else {
            guards.push(Guard::block("is a build folder", ""));
        }
        if request.confirm_build_id == request.build_id {
            guards.push(Guard::pass("confirmation matches", ""));
        } else {
            guards.push(Guard::block("confirmation matches", ""));
        }
        guards
    }

    const BUILD: &str = "019ff742-decd-4bfc-8df3-0f4fdea6ea26";

    #[test]
    fn shared_folders_are_not_deletable() {
        for name in ["legacy", "docker-cache", "debs", "test_data", ".."] {
            let guards = guards_of(&request(name, name, true));
            assert!(
                !guards[0].passed,
                "{name} must not be treated as a build folder"
            );
        }
    }

    #[test]
    fn a_build_uuid_is_deletable_in_principle() {
        let guards = guards_of(&request(BUILD, BUILD, true));
        assert!(guards.iter().all(|g| g.passed));
    }

    #[test]
    fn a_missing_or_wrong_confirmation_blocks() {
        assert!(!guards_of(&request(BUILD, "", true))[1].passed);
        assert!(!guards_of(&request(BUILD, "019ff742", true))[1].passed);
    }

    #[test]
    fn deletion_is_a_dry_run_unless_asked_otherwise() {
        let parsed: DeleteRequest =
            serde_json::from_str(&format!(r#"{{"build_id":"{BUILD}"}}"#)).unwrap();
        assert!(parsed.dry_run, "omitting dry_run must not delete");
        assert_eq!(parsed.confirm_build_id, "");
    }

    #[test]
    fn human_sizes_round_trip_well_enough_to_sort() {
        assert_eq!(bytes_to_human(0), "0B");
        assert_eq!(bytes_to_human(1024), "1.0K");
        assert_eq!(bytes_to_human(28 * 1024 * 1024 * 1024), "28.0G");
        assert_eq!(bytes_to_human(2 * 1024u64.pow(4)), "2.0T");
    }
}
