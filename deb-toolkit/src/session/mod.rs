//! Transactional .deb modification: open a package into a session directory,
//! mutate its control fields or data files, then repack into a fresh .deb.
//!
//! Mirrors the `scripts/debian/session/deb-session-*.sh` family on the mina
//! `develop` branch.

mod compression;
mod control;
mod data;
mod metadata;
mod open;
mod save;

pub use compression::Compression;
pub use metadata::Metadata;
pub use open::open;
pub use save::save;

use crate::misc::check_file_exists;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// A live session sitting on disk at `dir`. Construct via [`open`] (to take a
/// fresh .deb apart) or [`load`] (to pick up an already-extracted session).
pub struct Session {
    pub dir: PathBuf,
    pub metadata: Metadata,
}

impl Session {
    /// Validate that `dir` is a session directory previously created by
    /// [`open`], load its metadata, and return the handle.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        if !dir.is_dir() {
            return Err(anyhow!(
                "Session directory not found: {}\n\
                 \n\
                 Open a session first: deb-toolkit session open <input.deb> <session-dir>",
                dir.display()
            ));
        }
        let dir = dir
            .canonicalize()
            .with_context(|| format!("Could not resolve session directory: {}", dir.display()))?;
        let meta_file = dir.join("metadata.env");
        check_file_exists(meta_file.to_str().unwrap_or("")).map_err(|_| {
            anyhow!(
                "Session metadata not found: {}\n\
                 This doesn't appear to be a valid session directory.",
                meta_file.display()
            )
        })?;
        let metadata = Metadata::load(&meta_file)?;
        if !dir.join("data").is_dir() {
            return Err(anyhow!(
                "Session data directory missing: {}/data — session is corrupted",
                dir.display()
            ));
        }
        Ok(Session { dir, metadata })
    }

    /// `<session>/control/` (extracted control.tar.*).
    pub fn control_dir(&self) -> PathBuf {
        self.dir.join("control")
    }

    /// `<session>/data/` (extracted data.tar.*; modify this).
    pub fn data_dir(&self) -> PathBuf {
        self.dir.join("data")
    }

    /// `<session>/control/control` (the RFC822 metadata file).
    pub fn control_file(&self) -> PathBuf {
        self.control_dir().join("control")
    }

    /// Map a package path like `/var/lib/coda/foo` to a real filesystem path
    /// inside `<session>/data/`. Refuses to resolve outside the data dir.
    pub fn resolve_package_path(&self, pkg_path: &str) -> Result<PathBuf> {
        let stripped = pkg_path.strip_prefix('/').unwrap_or(pkg_path);
        let candidate = self.data_dir().join(stripped);
        // Normalize using std::path components — no symlink resolution
        // because the file may not exist yet (e.g. for `insert`).
        let normalized = normalize_path(&candidate);
        let data_dir = self.data_dir();
        if !normalized.starts_with(&data_dir) {
            return Err(anyhow!(
                "Path escapes session data directory: {}\n  normalized: {}\n  expected within: {}",
                pkg_path,
                normalized.display(),
                data_dir.display()
            ));
        }
        Ok(normalized)
    }

    // --- control mutations (see control.rs) ---

    pub fn read_field(&self, field: &str) -> Result<String> {
        control::read_field(&self.control_file(), field)
    }

    pub fn set_field(&self, field: &str, value: &str) -> Result<()> {
        control::set_field(&self.control_file(), field, value)
    }

    pub fn rename_package(&self, new_name: &str) -> Result<()> {
        self.set_field("Package", new_name)
    }

    pub fn replace_suite(&self, new_suite: &str) -> Result<()> {
        self.set_field("Suite", new_suite)
    }

    pub fn reversion(&self, new_version: &str, update_deps: bool) -> Result<()> {
        let old_version = self.read_field("Version").ok();
        self.set_field("Version", new_version)?;
        if update_deps {
            if let Some(old) = old_version.as_deref() {
                control::update_deps(&self.control_file(), old, new_version)?;
            }
        }
        Ok(())
    }

    // --- data mutations (see data.rs) ---

    pub fn insert(&self, dest: &str, sources: &[PathBuf], as_directory: bool) -> Result<()> {
        data::insert(self, dest, sources, as_directory)
    }

    pub fn remove(&self, pattern: &str) -> Result<usize> {
        data::remove(self, pattern)
    }

    pub fn move_path(&self, src: &str, dest: &str) -> Result<()> {
        data::move_path(self, src, dest)
    }

    pub fn replace(&self, pattern: &str, replacement: &Path) -> Result<usize> {
        data::replace(self, pattern, replacement)
    }
}

/// Lexical (symlink-unaware) path normalization that also works for paths
/// that don't exist yet.
fn normalize_path(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_dot_and_pops_dotdot() {
        assert_eq!(
            normalize_path(Path::new("/a/./b/../c/d")),
            PathBuf::from("/a/c/d")
        );
    }
}
