use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

/// Metadata about a file or directory in the cache.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: DateTime<Utc>,
}

/// Represents a debian package found in the cache.
#[derive(Debug, Clone, PartialEq)]
pub struct DebianEntry {
    pub name: String,
    pub codename: String,
    pub architecture: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: DateTime<Utc>,
}

/// Known Debian codenames.
pub const KNOWN_CODENAMES: &[&str] = &["bullseye", "focal", "noble", "jammy", "bookworm"];

/// Known Debian architectures.
pub const KNOWN_ARCHITECTURES: &[&str] = &["amd64", "arm64", "all"];

/// Trait abstracting cache storage operations.
/// Implementations can target the real filesystem or an in-memory mock.
pub trait CacheBackend: Send + Sync {
    /// List entries (files and dirs) directly under `path`.
    fn list_dir(&self, path: &Path) -> Result<Vec<CacheEntry>>;

    /// Recursively list all files under `path`.
    fn list_recursive(&self, path: &Path) -> Result<Vec<CacheEntry>>;

    /// Check if a path exists.
    fn exists(&self, path: &Path) -> bool;

    /// Check if a path is a directory.
    fn is_dir(&self, path: &Path) -> bool;

    /// Remove a directory and all its contents.
    fn remove_dir_all(&self, path: &Path) -> Result<()>;

    /// Create directories recursively.
    fn create_dir_all(&self, path: &Path) -> Result<()>;

    /// Copy a file or directory from `src` to `dst`.
    /// If `force` is false, do not overwrite existing files.
    fn copy(&self, src: &Path, dst: &Path, force: bool) -> Result<()>;

    /// Copy files matching a glob pattern from `pattern` to `dst`.
    fn copy_glob(&self, pattern: &str, dst: &Path, force: bool) -> Result<()>;
}

/// Real filesystem-backed cache backend.
pub struct FsBackend;

impl CacheBackend for FsBackend {
    fn list_dir(&self, path: &Path) -> Result<Vec<CacheEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let modified: DateTime<Utc> = metadata.modified()?.into();
            entries.push(CacheEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry.path(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn list_recursive(&self, path: &Path) -> Result<Vec<CacheEntry>> {
        let mut entries = Vec::new();
        self.collect_recursive(path, &mut entries)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        std::fs::create_dir_all(path)?;
        Ok(())
    }

    /// `cp -R` semantics: a file or directory copied onto an existing
    /// directory lands inside it under its own name; otherwise `dst` is the
    /// new path.
    fn copy(&self, src: &Path, dst: &Path, force: bool) -> Result<()> {
        let target = if dst.is_dir() {
            dst.join(
                src.file_name()
                    .ok_or_else(|| anyhow::anyhow!("Source has no file name: {}", src.display()))?,
            )
        } else {
            dst.to_path_buf()
        };
        if src.is_dir() {
            copy_dir_recursive(src, &target, force)
        } else {
            copy_file_atomic(src, &target, force)
        }
    }

    fn copy_glob(&self, pattern: &str, dst: &Path, force: bool) -> Result<()> {
        // Shell globbing: `*` does not match a leading dot.
        let options = glob::MatchOptions {
            require_literal_leading_dot: true,
            ..Default::default()
        };
        let paths: Vec<_> =
            glob::glob_with(pattern, options)?.collect::<std::result::Result<Vec<_>, _>>()?;
        if paths.is_empty() {
            anyhow::bail!("No files matched pattern: {}", pattern);
        }
        for path in paths {
            self.copy(&path, dst, force)?;
        }
        Ok(())
    }
}

impl FsBackend {
    fn collect_recursive(&self, path: &Path, entries: &mut Vec<CacheEntry>) -> Result<()> {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let modified: DateTime<Utc> = metadata.modified()?.into();
            let cache_entry = CacheEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry.path(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified,
            };
            if metadata.is_dir() {
                entries.push(cache_entry);
                self.collect_recursive(&entry.path(), entries)?;
            } else {
                entries.push(cache_entry);
            }
        }
        Ok(())
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path, force: bool) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.metadata()?.is_dir() {
            copy_dir_recursive(&src_path, &dst_path, force)?;
        } else {
            copy_file_atomic(&src_path, &dst_path, force)?;
        }
    }
    Ok(())
}

/// Copy to a temporary sibling, then rename over `dst`. Concurrent readers
/// never see a partial file, and concurrent writers of the same path cannot
/// interleave: the last rename wins with a complete file.
fn copy_file_atomic(src: &Path, dst: &Path, force: bool) -> Result<()> {
    if !force && dst.exists() {
        anyhow::bail!("File already exists: {}", dst.display());
    }
    let name = dst
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Destination has no file name: {}", dst.display()))?;
    let tmp = dst.with_file_name(format!(
        ".{}.tmp.{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    if let Err(e) = std::fs::copy(src, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!(
            "Failed to copy {} -> {}: {e}",
            src.display(),
            dst.display()
        ));
    }
    // Plain SFTP rename refuses an existing target (sshfs without the
    // posix-rename extension); remove it and retry once.
    let renamed = std::fs::rename(&tmp, dst).or_else(|e| {
        if dst.is_file() {
            std::fs::remove_file(dst).and_then(|_| std::fs::rename(&tmp, dst))
        } else {
            Err(e)
        }
    });
    if let Err(e) = renamed {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!(
            "Failed to move {} into place: {e}",
            dst.display()
        ));
    }
    Ok(())
}
