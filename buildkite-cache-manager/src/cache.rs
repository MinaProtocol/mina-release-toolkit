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

    fn copy(&self, src: &Path, dst: &Path, force: bool) -> Result<()> {
        if src.is_dir() {
            copy_dir_recursive(src, dst, force)?;
        } else {
            let dst_file = if dst.is_dir() {
                dst.join(src.file_name().unwrap_or_default())
            } else {
                dst.to_path_buf()
            };
            if !force && dst_file.exists() {
                anyhow::bail!("File already exists: {}", dst_file.display());
            }
            std::fs::copy(src, &dst_file)?;
        }
        Ok(())
    }

    fn copy_glob(&self, pattern: &str, dst: &Path, force: bool) -> Result<()> {
        let paths: Vec<_> = glob::glob(pattern)?.collect::<std::result::Result<Vec<_>, _>>()?;
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
            if !force && dst_path.exists() {
                anyhow::bail!("File already exists: {}", dst_path.display());
            }
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
