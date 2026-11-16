//! CI cache adapter.
//!
//! Mina's pipelines do not upload `.deb` files to Buildkite. They put them in
//! the shared cache on the Hetzner storage box, laid out as
//!
//! ```text
//! <root>/<buildkite-build-uuid>/debians/<codename>/<package>_<version>_<arch>.deb
//! ```
//!
//! which is why `USE_ARTIFACTS_FROM_BUILDKITE_BUILD` takes a build UUID. This
//! adapter answers: for a build, which packages are still in the cache.
//!
//! Note that the architecture is part of the filename, not a directory. The
//! `buildkite-cache-manager` README documents an extra `<arch>/` level; the
//! cache observed in August 2026 has no such level. Both shapes are accepted,
//! since a producer may yet write either.
//!
//! Two routes reach the same cache. In CI it is a mounted path; from a
//! workstation it is ssh. Both are supported, and neither is committed to
//! this repository — see `projects.example.yaml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{OpsError, OpsResult};
use crate::model::Presence;

/// One `.deb` found in the cache.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedDeb {
    pub codename: String,
    pub arch: String,
    pub filename: String,
    /// Parsed from the filename, which is `<package>_<version>.deb`.
    pub package: Option<String>,
    pub version: Option<String>,
}

/// One top-level entry in the cache root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheEntry {
    pub name: String,
    /// As `du -h` reports it. `None` when the route cannot measure cheaply.
    pub size: Option<String>,
    /// Bytes, for sorting. Parsed from the human figure, so it is approximate.
    pub size_bytes: Option<u64>,
    /// Last modification, as `ls -lt` reports it.
    pub modified: Option<String>,
    /// Whether the name is a Buildkite build UUID. The cache also holds
    /// `legacy`, `docker-cache` and other shared folders, which are not
    /// builds and must never be treated as one.
    pub is_build: bool,
}

/// One image tarball in `docker-cache`.
///
/// The cache stores images as `<image>/<tag>.tar.zst`, where the tag is the
/// docker tag with its parts joined by hyphens and the short commit first.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedImage {
    pub image: String,
    pub filename: String,
    /// The filename without `.tar.zst` — the docker tag it restores as.
    pub tag: String,
    pub commit: Option<String>,
    pub codename: Option<String>,
    pub arch: Option<String>,
    /// Whatever sits between the codename and the architecture: the network,
    /// and any profile such as `generic`.
    pub variant: Option<String>,
    pub size: Option<String>,
    pub size_bytes: Option<u64>,
    pub modified: Option<String>,
}

/// What was found under one build's cache folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedBuild {
    pub build_id: String,
    pub build_number: Option<u64>,
    pub pipeline: Option<String>,
    /// Where the lookup went, so a surprising answer can be traced.
    pub source: String,
    pub presence: Presence,
    pub debians: Vec<CachedDeb>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshCache {
    pub host: String,
    pub user: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Private key. Optional: the agent may already hold it.
    #[serde(default)]
    pub key: Option<String>,
    pub root: String,
}

fn default_ssh_port() -> u16 {
    22
}

/// How to reach the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheRoute {
    /// A mounted path, as in CI.
    Local(PathBuf),
    Ssh(SshCache),
}

impl CacheRoute {
    pub fn describe(&self) -> String {
        match self {
            CacheRoute::Local(path) => path.display().to_string(),
            // The host is the operator's own configuration, not a secret, but
            // the key path is never echoed.
            CacheRoute::Ssh(ssh) => format!("ssh {}@{}:{}", ssh.user, ssh.host, ssh.root),
        }
    }
}

/// Cache settings for one project. Deliberately optional: this repository is
/// public, so the real host and mount point come from the operator's own
/// configuration or the environment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheConfig {
    /// A mounted path, as CI has.
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub ssh: Option<SshCache>,
}

/// Chooses how to reach the cache.
///
/// The environment wins over configuration. A mounted path is preferred, but
/// only when it actually holds something: an empty mount point means the
/// share is not mounted, and ssh will answer correctly where the empty path
/// would answer "nothing cached".
pub fn resolve_route(config: Option<&CacheConfig>) -> Option<CacheRoute> {
    let env_root = std::env::var("MINA_OPS_CACHE_ROOT")
        .ok()
        .or_else(|| std::env::var("CACHE_BASE_URL").ok())
        .filter(|value| !value.trim().is_empty());
    if let Some(root) = env_root {
        return Some(CacheRoute::Local(PathBuf::from(root)));
    }

    if let Some(ssh) = ssh_from_environment() {
        return Some(CacheRoute::Ssh(ssh));
    }

    let config = config?;
    let usable_local = config
        .root
        .as_ref()
        .map(PathBuf::from)
        .filter(|root| has_entries(root));

    match (usable_local, &config.ssh) {
        (Some(root), _) => Some(CacheRoute::Local(root)),
        (None, Some(ssh)) => Some(CacheRoute::Ssh(ssh.clone())),
        // Configured but unusable: report through the local route so the
        // reason reaches the caller rather than vanishing.
        (None, None) => config.root.as_ref().map(|r| CacheRoute::Local(r.into())),
    }
}

fn ssh_from_environment() -> Option<SshCache> {
    let host = std::env::var("MINA_OPS_CACHE_SSH_HOST").ok()?;
    let user = std::env::var("MINA_OPS_CACHE_SSH_USER").ok()?;
    let root = std::env::var("MINA_OPS_CACHE_SSH_ROOT").ok()?;
    Some(SshCache {
        host,
        user,
        port: std::env::var("MINA_OPS_CACHE_SSH_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(default_ssh_port),
        key: std::env::var("MINA_OPS_CACHE_SSH_KEY").ok(),
        root,
    })
}

fn has_entries(root: &Path) -> bool {
    std::fs::read_dir(root)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

pub struct CacheClient {
    route: CacheRoute,
}

impl CacheClient {
    pub fn new(route: CacheRoute) -> Self {
        Self { route }
    }

    pub fn route(&self) -> &CacheRoute {
        &self.route
    }

    /// A name that may be interpolated into a remote command.
    ///
    /// The cache root holds shared folders as well as builds, so lookups take
    /// arbitrary names. Only a plain filename is ever accepted: no slashes,
    /// no `..`, nothing the shell would treat as syntax.
    pub fn is_safe_entry_name(name: &str) -> bool {
        !name.is_empty()
            && name != "."
            && name != ".."
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    }

    /// Packages under any cache entry, build or shared folder.
    ///
    /// A build keeps its packages under `<build>/debians/<codename>/`, and so
    /// does `legacy`; `debs` puts codenames directly under itself. All three
    /// are handled, because `legacy` is the folder people most often need to
    /// look inside.
    pub async fn debians_for_entry(&self, entry: &str) -> (Presence, Vec<CachedDeb>) {
        if !Self::is_safe_entry_name(entry) {
            return (
                Presence::Unknown(format!("'{entry}' is not a valid cache entry name")),
                Vec::new(),
            );
        }
        if looks_like_build_id(entry) {
            return self.debians_for_build(entry).await;
        }
        match &self.route {
            CacheRoute::Ssh(ssh) => {
                let base = format!("{}/{}", ssh.root.trim_end_matches('/'), entry);
                self.ssh_lookup_at(ssh, &base).await
            }
            CacheRoute::Local(root) => {
                let mut found = Vec::new();
                collect_debs(&root.join(entry), "", &mut found);
                finish(found)
            }
        }
    }

    /// Packages cached for one Buildkite build.
    ///
    /// The three outcomes are kept apart deliberately: a build folder that is
    /// absent, a build folder that holds no packages, and a lookup that could
    /// not run.
    pub async fn debians_for_build(&self, build_id: &str) -> (Presence, Vec<CachedDeb>) {
        match &self.route {
            CacheRoute::Local(root) => self.local_lookup(root, build_id).await,
            CacheRoute::Ssh(ssh) => self.ssh_lookup(ssh, build_id).await,
        }
    }

    /// Everything in the cache root, with sizes and dates.
    ///
    /// Over ssh this is two commands and about a second for the whole cache,
    /// however large it is, because `du --max-depth=1` and `ls -lt` each walk
    /// the top level once.
    pub async fn list_entries(&self) -> OpsResult<Vec<CacheEntry>> {
        match &self.route {
            CacheRoute::Ssh(ssh) => self.ssh_list(ssh).await,
            CacheRoute::Local(root) => self.local_list(root),
        }
    }

    async fn ssh_list(&self, ssh: &SshCache) -> OpsResult<Vec<CacheEntry>> {
        let root = ssh.root.trim_end_matches('/').to_string();

        let sizes = self
            .ssh_command(ssh, &format!("du -h --max-depth=1 {root}"))
            .await?;
        let dates = self.ssh_command(ssh, &format!("ls -lt {root}")).await?;

        let mut entries = parse_du(&sizes, &root);
        let modified = parse_ls_times(&dates);
        for entry in &mut entries {
            entry.modified = modified.get(&entry.name).cloned();
        }
        entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
        Ok(entries)
    }

    fn local_list(&self, root: &Path) -> OpsResult<Vec<CacheEntry>> {
        // Sizes are deliberately not computed here: walking a multi-terabyte
        // tree to add a column would be far more expensive than the listing
        // itself, and `None` says so honestly.
        let read = std::fs::read_dir(root)
            .map_err(|e| OpsError::Other(format!("cannot read {}: {e}", root.display())))?;
        let mut entries: Vec<CacheEntry> = read
            .flatten()
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                CacheEntry {
                    is_build: looks_like_build_id(&name),
                    name,
                    size: None,
                    size_bytes: None,
                    modified: None,
                }
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// Image tarballs under a cache entry, `docker-cache` in practice.
    ///
    /// One `ls -lR` returns every image with its size and date, because the
    /// tree is two levels deep and holds tens of files rather than thousands.
    pub async fn images_for_entry(&self, entry: &str) -> OpsResult<Vec<CachedImage>> {
        if !Self::is_safe_entry_name(entry) {
            return Err(OpsError::Other(format!(
                "'{entry}' is not a valid cache entry name"
            )));
        }
        match &self.route {
            CacheRoute::Ssh(ssh) => {
                let base = format!("{}/{}", ssh.root.trim_end_matches('/'), entry);
                let output = self.ssh_command(ssh, &format!("ls -lR {base}")).await?;
                Ok(parse_image_listing(&output))
            }
            CacheRoute::Local(root) => Ok(local_images(&root.join(entry))),
        }
    }

    /// Removes one build's folder. The caller is responsible for the checks;
    /// this only refuses what is structurally wrong.
    pub async fn delete_build(&self, build_id: &str) -> OpsResult<()> {
        if !looks_like_build_id(build_id) {
            return Err(OpsError::Other(format!(
                "'{build_id}' is not a build UUID. Only build folders can be removed; shared \
                 folders such as legacy and docker-cache are never touched."
            )));
        }

        match &self.route {
            CacheRoute::Ssh(ssh) => {
                let root = ssh.root.trim_end_matches('/');
                self.ssh_command(ssh, &format!("rm -r {root}/{build_id}"))
                    .await
                    .map(|_| ())
            }
            CacheRoute::Local(root) => std::fs::remove_dir_all(root.join(build_id))
                .map_err(|e| OpsError::Other(format!("cannot remove {build_id}: {e}"))),
        }
    }

    async fn ssh_command(&self, ssh: &SshCache, remote: &str) -> OpsResult<String> {
        let mut command = Command::new("ssh");
        command
            .arg("-p")
            .arg(ssh.port.to_string())
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg("-o")
            .arg("ConnectTimeout=20");
        if let Some(key) = &ssh.key {
            command.arg("-i").arg(expand_home(key));
        }
        let output = command
            .arg(format!("{}@{}", ssh.user, ssh.host))
            .arg(remote)
            .output()
            .await
            .map_err(|e| OpsError::Other(format!("could not run ssh: {e}")))?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            return Err(OpsError::Other(format!(
                "the cache refused '{remote}': {}",
                first_line(&stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn local_lookup(&self, root: &Path, build_id: &str) -> (Presence, Vec<CachedDeb>) {
        if !root.exists() {
            return (
                Presence::Unknown(format!("cache root {} does not exist", root.display())),
                Vec::new(),
            );
        }
        // An empty root almost always means the share is not mounted. Calling
        // that "no packages" would send somebody to rebuild artifacts that are
        // sitting on the storage box.
        match std::fs::read_dir(root).map(|mut entries| entries.next().is_none()) {
            Ok(true) => {
                return (
                    Presence::Unknown(format!(
                        "cache root {} is empty, so it is probably not mounted",
                        root.display()
                    )),
                    Vec::new(),
                )
            }
            Err(e) => {
                return (
                    Presence::Unknown(format!("cannot read {}: {e}", root.display())),
                    Vec::new(),
                )
            }
            Ok(false) => {}
        }

        let debians_dir = root.join(build_id).join("debians");
        if !debians_dir.exists() {
            return (Presence::Missing, Vec::new());
        }

        let mut found = Vec::new();
        collect_debs(&debians_dir, "", &mut found);
        finish(found)
    }

    async fn ssh_lookup(&self, ssh: &SshCache, build_id: &str) -> (Presence, Vec<CachedDeb>) {
        let base = format!("{}/{}/debians", ssh.root.trim_end_matches('/'), build_id);
        self.ssh_lookup_at(ssh, &base).await
    }

    async fn ssh_lookup_at(&self, ssh: &SshCache, base: &str) -> (Presence, Vec<CachedDeb>) {
        // The Hetzner storage box runs a restricted shell: it accepts a
        // command with arguments, but no `cd`, no `&&`, and no glob
        // expansion. `ls -R` is therefore the only single round trip that
        // reaches the files.
        let remote = format!("ls -R {base}");

        let mut command = Command::new("ssh");
        command
            .arg("-p")
            .arg(ssh.port.to_string())
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg("-o")
            .arg("ConnectTimeout=20");
        if let Some(key) = &ssh.key {
            command.arg("-i").arg(expand_home(key));
        }
        command
            .arg(format!("{}@{}", ssh.user, ssh.host))
            .arg(&remote);

        let output = match command.output().await {
            Ok(output) => output,
            Err(e) => {
                return (
                    Presence::Unknown(format!("could not run ssh: {e}")),
                    Vec::new(),
                )
            }
        };

        // `cd` fails when the build folder is absent, and the pipeline then
        // exits non-zero with no output. That is a real "not cached", but an
        // authentication or network failure writes to stderr — the two must
        // not be confused.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            // `ls` says exactly this when the build was never cached or has
            // been pruned. Any other message is a failure to look, not an
            // absence.
            if stderr.contains("No such file or directory") {
                return (Presence::Missing, Vec::new());
            }
            return (
                Presence::Unknown(format!("ssh to the cache failed: {}", first_line(&stderr))),
                Vec::new(),
            );
        }

        finish(parse_recursive_listing(
            &String::from_utf8_lossy(&output.stdout),
            base,
        ))
    }
}

/// Walks `debians/`, gathering every `.deb` with the path it sits at.
fn collect_debs(dir: &Path, relative: &str, found: &mut Vec<CachedDeb>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if path.is_dir() {
            let deeper = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            collect_debs(&path, &deeper, found);
        } else if name.ends_with(".deb") && !relative.is_empty() {
            let relative = relative.strip_prefix("debians/").unwrap_or(relative);
            found.push(cached_deb_at(relative, &name));
        }
    }
}

fn finish(found: Vec<CachedDeb>) -> (Presence, Vec<CachedDeb>) {
    if found.is_empty() {
        (Presence::Missing, found)
    } else {
        (Presence::Present, found)
    }
}

/// Output of `ls -R <base>`: directory headers ending in `:`, then that
/// directory's entries.
///
/// Paths are reported relative to `base`, and a leading `debians/` is dropped
/// so a build folder, `legacy` and `debs` all yield the codename first.
pub fn parse_recursive_listing(listing: &str, base: &str) -> Vec<CachedDeb> {
    let base = base.trim_end_matches('/');
    let mut found = Vec::new();
    let mut current = String::new();

    for line in listing.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_suffix(':') {
            current = relative_to(header.trim(), base);
            continue;
        }
        if line.ends_with(".deb") && !current.is_empty() {
            found.push(cached_deb_at(&current, line));
        }
    }

    found
}

/// The part of `header` below `base`, with any `debians` level removed.
fn relative_to(header: &str, base: &str) -> String {
    let rest = header
        .strip_prefix(base)
        .map(|rest| rest.trim_start_matches('/'))
        // `ls` may report a path differently from the one asked for; fall
        // back to the old rule so a build folder still resolves.
        .or_else(|| {
            header
                .rsplit_once("/debians")
                .map(|(_, r)| r.trim_start_matches('/'))
        })
        .unwrap_or("");

    match rest.strip_prefix("debians") {
        Some(below) => below.trim_start_matches('/').to_string(),
        None => rest.to_string(),
    }
}

/// A `.deb` at a path below `debians`, which is either `<codename>` or
/// `<codename>/<arch>`.
pub fn cached_deb_at(relative_dir: &str, filename: &str) -> CachedDeb {
    let mut segments = relative_dir.split('/').filter(|s| !s.is_empty());
    let codename = segments.next().unwrap_or("unknown").to_string();
    let arch_directory = segments.next().map(str::to_string);

    let mut deb = parse_cached_deb(&codename, "", filename);
    deb.arch = arch_directory
        .or_else(|| arch_from_filename(filename))
        .unwrap_or_else(|| "unknown".to_string());
    deb
}

/// Debian filenames are `<package>_<version>_<arch>.deb`. Older entries omit
/// the architecture, in which case it cannot be known from the name alone.
pub fn arch_from_filename(filename: &str) -> Option<String> {
    let stem = filename.strip_suffix(".deb")?;
    let fields: Vec<&str> = stem.split('_').collect();
    // `<package>_<version>_<arch>`. With fewer fields the architecture is
    // simply not in the name, and the version must not be mistaken for it.
    if fields.len() < 3 {
        return None;
    }
    fields
        .last()
        .filter(|arch| !arch.is_empty())
        .map(|arch| arch.to_string())
}

/// Debian filenames are `<package>_<version>.deb`, sometimes with an
/// architecture segment. Anything that does not parse keeps its filename and
/// reports no package, rather than guessing.
pub fn parse_cached_deb(codename: &str, arch: &str, filename: &str) -> CachedDeb {
    let stem = filename.trim_end_matches(".deb");
    let mut fields = stem.split('_');
    let package = fields.next().filter(|p| !p.is_empty()).map(str::to_string);
    let version = fields.next().filter(|v| !v.is_empty()).map(str::to_string);

    CachedDeb {
        codename: codename.to_string(),
        arch: arch.to_string(),
        filename: filename.to_string(),
        package,
        version,
    }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// Debian codenames that appear in an image tag.
const IMAGE_CODENAMES: [&str; 6] = ["bullseye", "bookworm", "focal", "jammy", "noble", "buster"];
const IMAGE_ARCHES: [&str; 2] = ["amd64", "arm64"];
const IMAGE_SUFFIX: &str = ".tar.zst";

/// `ls -lR` output: a directory header, then its long-format entries.
pub fn parse_image_listing(output: &str) -> Vec<CachedImage> {
    let mut images = Vec::new();
    let mut current = String::new();

    for line in output.lines() {
        let line = line.trim_end();
        if let Some(header) = line.strip_suffix(':') {
            current = header
                .trim()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            continue;
        }
        // Only regular files; directories and the `total` line are not images.
        if !line.starts_with('-') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        let filename = fields[8..].join(" ");
        if !filename.ends_with(IMAGE_SUFFIX) {
            continue;
        }
        let size_bytes: Option<u64> = fields[4].parse().ok();
        images.push(image_from_filename(
            &current,
            &filename,
            size_bytes,
            Some(format!("{} {} {}", fields[5], fields[6], fields[7])),
        ));
    }

    images
}

fn local_images(dir: &Path) -> Vec<CachedImage> {
    let mut images = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return images;
    };
    for image_dir in entries.flatten() {
        if !image_dir.path().is_dir() {
            continue;
        }
        let image = image_dir.file_name().to_string_lossy().to_string();
        let Ok(files) = std::fs::read_dir(image_dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let filename = file.file_name().to_string_lossy().to_string();
            if !filename.ends_with(IMAGE_SUFFIX) {
                continue;
            }
            let size = file.metadata().ok().map(|m| m.len());
            images.push(image_from_filename(&image, &filename, size, None));
        }
    }
    images
}

/// Splits `<commit>-<codename>-<variant…>-<arch>` without inventing parts it
/// cannot find: an unrecognised segment stays in `variant` rather than being
/// guessed at.
pub fn image_from_filename(
    image: &str,
    filename: &str,
    size_bytes: Option<u64>,
    modified: Option<String>,
) -> CachedImage {
    let tag = filename.trim_end_matches(IMAGE_SUFFIX).to_string();
    let mut parts: Vec<&str> = tag.split('-').collect();

    let commit = parts
        .first()
        .filter(|p| p.len() >= 6 && p.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|p| p.to_string());
    if commit.is_some() {
        parts.remove(0);
    }

    let arch = parts
        .last()
        .filter(|p| IMAGE_ARCHES.contains(p))
        .map(|p| p.to_string());
    if arch.is_some() {
        parts.pop();
    }

    let codename = parts
        .iter()
        .position(|p| IMAGE_CODENAMES.contains(p))
        .map(|index| parts.remove(index).to_string());

    let variant = (!parts.is_empty()).then(|| parts.join("-"));

    CachedImage {
        image: image.to_string(),
        filename: filename.to_string(),
        tag,
        commit,
        codename,
        arch,
        variant,
        size: size_bytes.map(bytes_to_human_size),
        size_bytes,
        modified,
    }
}

fn bytes_to_human_size(bytes: u64) -> String {
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

/// A Buildkite build UUID, which is what a cache build folder is named after.
pub fn looks_like_build_id(name: &str) -> bool {
    let groups: Vec<&str> = name.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `du -h --max-depth=1` output: `<size>\t<path>`. The root's own total is
/// dropped, since it is not an entry.
pub fn parse_du(output: &str, root: &str) -> Vec<CacheEntry> {
    output
        .lines()
        .filter_map(|line| {
            let (size, path) = line.split_once('\t')?;
            let path = path.trim();
            if path == root || path == format!("{root}/") {
                return None;
            }
            let name = path.rsplit('/').next()?.to_string();
            if name.is_empty() {
                return None;
            }
            Some(CacheEntry {
                is_build: looks_like_build_id(&name),
                name,
                size_bytes: human_size_to_bytes(size.trim()),
                size: Some(size.trim().to_string()),
                modified: None,
            })
        })
        .collect()
}

/// `ls -lt` output, mapping a name to the date columns.
pub fn parse_ls_times(output: &str) -> std::collections::HashMap<String, String> {
    let mut times = std::collections::HashMap::new();
    for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // permissions links owner group size month day time name
        if fields.len() < 9 || !fields[0].starts_with('d') {
            continue;
        }
        let name = fields[8..].join(" ");
        times.insert(name, format!("{} {} {}", fields[5], fields[6], fields[7]));
    }
    times
}

/// `28G`, `705M`, `1.2T` — approximate, and only ever used for sorting and
/// for telling somebody roughly how much a deletion would free.
pub fn human_size_to_bytes(value: &str) -> Option<u64> {
    let value = value.trim();
    let (number, unit) = value.split_at(value.find(|c: char| c.is_ascii_alphabetic())?);
    let number: f64 = number.parse().ok()?;
    let multiplier: f64 = match unit.chars().next()? {
        'K' | 'k' => 1024.0,
        'M' => 1024f64.powi(2),
        'G' => 1024f64.powi(3),
        'T' => 1024f64.powi(4),
        'P' => 1024f64.powi(5),
        _ => return None,
    };
    Some((number * multiplier) as u64)
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

    /// Real `ls -R` output, abbreviated: the architecture is in the
    /// filename and there is no architecture directory.
    const LS_R: &str = "/cache/019ff742/debians:\n\
                        bookworm\n\
                        noble\n\
                        \n\
                        /cache/019ff742/debians/bookworm:\n\
                        mina-archive-devnet_3.5.0-compatible-849edfa_amd64.deb\n\
                        mina-archive-devnet_3.5.0-compatible-849edfa_arm64.deb\n\
                        \n\
                        /cache/019ff742/debians/noble:\n\
                        mina-devnet_3.5.0-compatible-849edfa_amd64.deb\n\
                        notes.txt\n";

    #[test]
    fn recursive_listings_take_the_architecture_from_the_filename() {
        let debs = parse_recursive_listing(LS_R, "/cache/019ff742/debians");
        assert_eq!(debs.len(), 3, "only .deb files, and no directory names");

        let bookworm: Vec<_> = debs.iter().filter(|d| d.codename == "bookworm").collect();
        assert_eq!(bookworm.len(), 2);
        let arches: Vec<&str> = bookworm.iter().map(|d| d.arch.as_str()).collect();
        assert!(arches.contains(&"amd64") && arches.contains(&"arm64"));
        assert_eq!(bookworm[0].package.as_deref(), Some("mina-archive-devnet"));
        assert_eq!(
            bookworm[0].version.as_deref(),
            Some("3.5.0-compatible-849edfa")
        );
    }

    #[test]
    fn an_architecture_directory_is_honoured_when_a_producer_writes_one() {
        let listing = "/cache/b/debians/noble/arm64:\nmina-devnet_3.2.0-abc.deb\n";
        let debs = parse_recursive_listing(listing, "/cache/b/debians");
        assert_eq!(debs.len(), 1);
        assert_eq!(debs[0].codename, "noble");
        assert_eq!(
            debs[0].arch, "arm64",
            "the directory wins over the filename"
        );
    }

    #[test]
    fn entries_before_any_directory_header_are_ignored() {
        assert!(parse_recursive_listing("stray_1.0_amd64.deb\n", "/cache/b/debians").is_empty());
    }

    #[test]
    fn image_tags_split_into_commit_codename_variant_and_arch() {
        let image = image_from_filename(
            "mina-toolchain",
            "169fd52-bookworm-devnet-arm64.tar.zst",
            Some(1024 * 1024 * 1024),
            Some("Jul 27 15:51".into()),
        );
        assert_eq!(image.image, "mina-toolchain");
        assert_eq!(image.tag, "169fd52-bookworm-devnet-arm64");
        assert_eq!(image.commit.as_deref(), Some("169fd52"));
        assert_eq!(image.codename.as_deref(), Some("bookworm"));
        assert_eq!(image.arch.as_deref(), Some("arm64"));
        assert_eq!(image.variant.as_deref(), Some("devnet"));
        assert_eq!(image.size.as_deref(), Some("1.0G"));
    }

    #[test]
    fn an_absent_architecture_is_left_absent() {
        let image = image_from_filename(
            "mina-daemon",
            "12cec3a-bullseye-devnet-generic.tar.zst",
            None,
            None,
        );
        assert_eq!(image.arch, None, "amd64 is implied, never invented");
        assert_eq!(image.variant.as_deref(), Some("devnet-generic"));
        assert_eq!(image.size, None);
    }

    #[test]
    fn a_doubled_network_stays_visible_rather_than_being_tidied_away() {
        // Tags like this came from a real bug; hiding it would hide the bug.
        let image = image_from_filename(
            "mina-daemon",
            "214894f-bullseye-devnet-devnet-generic.tar.zst",
            None,
            None,
        );
        assert_eq!(image.variant.as_deref(), Some("devnet-devnet-generic"));
    }

    #[test]
    fn image_listings_read_sizes_and_skip_non_images() {
        let listing = "/cache/docker-cache/mina-daemon:\n\
                       total 623\n\
                       -rwxr--r-- 1 u1 u1 9308937114 Jul 27 15:51 12cec3a-bullseye-devnet-generic.tar.zst\n\
                       -rw-r--r-- 1 u1 u1 12 Jul 27 15:51 notes.txt\n\
                       drwxr-xr-x 2 u1 u1 4096 Jul 27 15:51 subdir\n";
        let images = parse_image_listing(listing);
        assert_eq!(images.len(), 1, "only .tar.zst files are images");
        assert_eq!(images[0].image, "mina-daemon");
        assert_eq!(images[0].size_bytes, Some(9_308_937_114));
        assert_eq!(images[0].size.as_deref(), Some("8.7G"));
        assert_eq!(images[0].modified.as_deref(), Some("Jul 27 15:51"));
    }

    #[test]
    fn the_legacy_folder_reads_like_a_build_folder() {
        // legacy/debians/<codename>/… — the `debians` level is dropped, so the
        // codename comes first exactly as it does for a build.
        let listing = "/cache/legacy:\ndebians\n\n\
                       /cache/legacy/debians:\nbullseye\n\n\
                       /cache/legacy/debians/bullseye:\n\
                       mina-devnet_3.3.0-compatible-4fa3a5b_amd64.deb\n";
        let debs = parse_recursive_listing(listing, "/cache/legacy");
        assert_eq!(debs.len(), 1);
        assert_eq!(debs[0].codename, "bullseye");
        assert_eq!(debs[0].arch, "amd64");
        assert_eq!(debs[0].version.as_deref(), Some("3.3.0-compatible-4fa3a5b"));
    }

    #[test]
    fn the_debs_folder_puts_codenames_directly_under_itself() {
        let listing = "/cache/debs:\nbookworm\n\n\
                       /cache/debs/bookworm:\nmina-devnet_1.0_amd64.deb\n";
        let debs = parse_recursive_listing(listing, "/cache/debs");
        assert_eq!(debs.len(), 1);
        assert_eq!(debs[0].codename, "bookworm");
    }

    #[test]
    fn only_plain_names_may_reach_a_remote_command() {
        assert!(CacheClient::is_safe_entry_name("legacy"));
        assert!(CacheClient::is_safe_entry_name(
            "019ff742-decd-4bfc-8df3-0f4fdea6ea26"
        ));
        assert!(CacheClient::is_safe_entry_name("test_data"));
        // Anything the shell or the path could reinterpret.
        for bad in ["..", ".", "", "a/b", "a b", "a;rm -rf /", "$(x)", "a*", "~"] {
            assert!(
                !CacheClient::is_safe_entry_name(bad),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn an_architecture_missing_from_the_filename_is_not_invented() {
        // Two fields only: the last one is the version, not an architecture.
        assert_eq!(arch_from_filename("mina-devnet_3.2.0-abc.deb"), None);
        assert_eq!(arch_from_filename("solo.deb"), None);
        assert_eq!(
            arch_from_filename("mina-devnet_3.2.0-abc_amd64.deb").as_deref(),
            Some("amd64")
        );

        let deb = cached_deb_at("noble", "mina-devnet_3.2.0-abc.deb");
        assert_eq!(
            deb.arch, "unknown",
            "an unknown architecture is not guessed"
        );
        assert_eq!(deb.version.as_deref(), Some("3.2.0-abc"));
    }

    #[test]
    fn an_unparseable_filename_keeps_its_name_and_claims_no_package() {
        let deb = parse_cached_deb("noble", "amd64", "weird.deb");
        assert_eq!(deb.filename, "weird.deb");
        assert_eq!(deb.package.as_deref(), Some("weird"));
        assert_eq!(deb.version, None);
    }

    #[tokio::test]
    async fn an_empty_cache_root_is_unknown_not_missing() {
        // This is the trap: an unmounted share looks exactly like an empty
        // one, and reporting "missing" would send somebody to rebuild.
        let dir = tempfile::tempdir().unwrap();
        let client = CacheClient::new(CacheRoute::Local(dir.path().to_path_buf()));
        let (presence, debs) = client.debians_for_build("019ff144").await;
        assert!(presence.is_unknown(), "got {presence:?}");
        assert!(debs.is_empty());
    }

    #[tokio::test]
    async fn a_missing_cache_root_is_unknown() {
        let client = CacheClient::new(CacheRoute::Local(PathBuf::from("/nope/not/here")));
        let (presence, _) = client.debians_for_build("019ff144").await;
        assert!(presence.is_unknown());
    }

    #[tokio::test]
    async fn a_populated_root_without_this_build_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("another-build").join("debians")).unwrap();
        let client = CacheClient::new(CacheRoute::Local(dir.path().to_path_buf()));
        let (presence, _) = client.debians_for_build("019ff144").await;
        assert_eq!(presence, Presence::Missing);
    }

    #[tokio::test]
    async fn packages_under_a_build_are_found_with_their_codename_and_arch() {
        let dir = tempfile::tempdir().unwrap();
        let codename_dir = dir.path().join("019ff144").join("debians").join("noble");
        std::fs::create_dir_all(&codename_dir).unwrap();
        std::fs::write(
            codename_dir.join("mina-devnet_3.2.0-49b523c_amd64.deb"),
            b"x",
        )
        .unwrap();
        std::fs::write(codename_dir.join("build.log"), b"x").unwrap();

        let client = CacheClient::new(CacheRoute::Local(dir.path().to_path_buf()));
        let (presence, debs) = client.debians_for_build("019ff144").await;
        assert_eq!(presence, Presence::Present);
        assert_eq!(debs.len(), 1, "only .deb files count");
        assert_eq!(debs[0].codename, "noble");
        assert_eq!(debs[0].arch, "amd64");
        assert_eq!(debs[0].version.as_deref(), Some("3.2.0-49b523c"));
    }

    #[test]
    fn the_route_description_never_echoes_the_key() {
        let route = CacheRoute::Ssh(SshCache {
            host: "example.your-storagebox.de".to_string(),
            user: "u1".to_string(),
            port: 23,
            key: Some("~/secrets/very-secret.key".to_string()),
            root: "/home/cache".to_string(),
        });
        let described = route.describe();
        assert!(!described.contains("very-secret"));
        assert!(described.contains("example.your-storagebox.de"));
    }

    #[test]
    fn home_relative_key_paths_are_expanded() {
        let expanded = expand_home("~/x/y.key");
        assert!(!expanded.to_string_lossy().starts_with('~'));
        assert_eq!(expand_home("/abs/y.key"), PathBuf::from("/abs/y.key"));
    }
}
