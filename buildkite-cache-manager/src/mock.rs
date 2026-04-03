//! In-memory mock of [`CacheBackend`] for testing.
//!
//! Simulates a Hetzner cache filesystem using a `HashMap` of paths to entries.
//! All operations (list, copy, remove, etc.) work against this in-memory tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::cache::{CacheBackend, CacheEntry};

#[derive(Debug, Clone)]
struct MockFile {
    is_dir: bool,
    size: u64,
    modified: DateTime<Utc>,
    #[allow(dead_code)]
    content: Vec<u8>,
}

/// In-memory mock of the Hetzner cache filesystem.
pub struct MockBackend {
    files: Mutex<HashMap<PathBuf, MockFile>>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
        }
    }

    /// Add a directory to the mock filesystem.
    pub fn add_dir(&self, path: &str, modified: DateTime<Utc>) {
        let mut files = self.files.lock().unwrap();
        files.insert(
            PathBuf::from(path),
            MockFile {
                is_dir: true,
                size: 0,
                modified,
                content: Vec::new(),
            },
        );
    }

    /// Add a file to the mock filesystem.
    pub fn add_file(&self, path: &str, size: u64, modified: DateTime<Utc>) {
        let mut files = self.files.lock().unwrap();
        // Ensure parent dirs exist
        let p = PathBuf::from(path);
        if let Some(parent) = p.parent() {
            self.ensure_parents_locked(&mut files, parent, modified);
        }
        files.insert(
            p,
            MockFile {
                is_dir: false,
                size,
                modified,
                content: vec![0; size as usize],
            },
        );
    }

    fn ensure_parents_locked(
        &self,
        files: &mut HashMap<PathBuf, MockFile>,
        path: &Path,
        modified: DateTime<Utc>,
    ) {
        let mut ancestors: Vec<PathBuf> = Vec::new();
        let mut current = path.to_path_buf();
        while current != PathBuf::from("/") && current != PathBuf::from("") {
            if !files.contains_key(&current) {
                ancestors.push(current.clone());
            }
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            } else {
                break;
            }
        }
        for ancestor in ancestors {
            files.insert(
                ancestor,
                MockFile {
                    is_dir: true,
                    size: 0,
                    modified,
                    content: Vec::new(),
                },
            );
        }
    }

    /// Check if a path was removed (useful for testing prune).
    pub fn path_exists(&self, path: &str) -> bool {
        self.files
            .lock()
            .unwrap()
            .contains_key(&PathBuf::from(path))
    }
}

impl CacheBackend for MockBackend {
    fn list_dir(&self, path: &Path) -> Result<Vec<CacheEntry>> {
        let files = self.files.lock().unwrap();
        let path = path.to_path_buf();

        if !files.contains_key(&path) {
            anyhow::bail!("Directory does not exist: {}", path.display());
        }

        let mut entries: Vec<CacheEntry> = files
            .iter()
            .filter(|(p, _)| {
                if let Some(parent) = p.parent() {
                    parent == path && *p != &path
                } else {
                    false
                }
            })
            .map(|(p, f)| CacheEntry {
                name: p.file_name().unwrap().to_string_lossy().to_string(),
                path: p.clone(),
                is_dir: f.is_dir,
                size: f.size,
                modified: f.modified,
            })
            .collect();

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn list_recursive(&self, path: &Path) -> Result<Vec<CacheEntry>> {
        let files = self.files.lock().unwrap();
        let path = path.to_path_buf();

        let mut entries: Vec<CacheEntry> = files
            .iter()
            .filter(|(p, _)| p.starts_with(&path) && *p != &path)
            .map(|(p, f)| CacheEntry {
                name: p.file_name().unwrap().to_string_lossy().to_string(),
                path: p.clone(),
                is_dir: f.is_dir,
                size: f.size,
                modified: f.modified,
            })
            .collect();

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.lock().unwrap().contains_key(&path.to_path_buf())
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.files
            .lock()
            .unwrap()
            .get(&path.to_path_buf())
            .map(|f| f.is_dir)
            .unwrap_or(false)
    }

    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        let path = path.to_path_buf();
        let to_remove: Vec<PathBuf> = files
            .keys()
            .filter(|p| p.starts_with(&path))
            .cloned()
            .collect();
        for p in to_remove {
            files.remove(&p);
        }
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        let modified = Utc::now();
        self.ensure_parents_locked(&mut files, path, modified);
        files.insert(
            path.to_path_buf(),
            MockFile {
                is_dir: true,
                size: 0,
                modified,
                content: Vec::new(),
            },
        );
        Ok(())
    }

    fn copy(&self, src: &Path, dst: &Path, force: bool) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        let src_file = files
            .get(&src.to_path_buf())
            .ok_or_else(|| anyhow::anyhow!("Source not found: {}", src.display()))?
            .clone();

        let dst_path = if files
            .get(&dst.to_path_buf())
            .map(|f| f.is_dir)
            .unwrap_or(false)
        {
            dst.join(src.file_name().unwrap_or_default())
        } else {
            dst.to_path_buf()
        };

        if !force && files.contains_key(&dst_path) {
            anyhow::bail!("File already exists: {}", dst_path.display());
        }

        files.insert(dst_path, src_file);
        Ok(())
    }

    fn copy_glob(&self, pattern: &str, dst: &Path, force: bool) -> Result<()> {
        let files = self.files.lock().unwrap();
        let glob_pattern = glob::Pattern::new(pattern)?;
        let matching: Vec<(PathBuf, MockFile)> = files
            .iter()
            .filter(|(p, f)| !f.is_dir && glob_pattern.matches_path(p))
            .map(|(p, f)| (p.clone(), f.clone()))
            .collect();
        drop(files);

        if matching.is_empty() {
            anyhow::bail!("No files matched pattern: {}", pattern);
        }

        for (src_path, _) in matching {
            self.copy(&src_path, dst, force)?;
        }
        Ok(())
    }
}
