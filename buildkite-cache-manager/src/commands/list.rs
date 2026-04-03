use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;
use tabled::{Table, Tabled};

use crate::cache::{CacheBackend, DebianEntry, KNOWN_ARCHITECTURES, KNOWN_CODENAMES};
use crate::cli::OutputFormat;

/// A displayable folder entry for table output.
#[derive(Tabled, Serialize)]
struct FolderRow {
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Type")]
    folder_type: String,
    #[tabled(rename = "Modified")]
    modified: String,
    #[tabled(rename = "Items")]
    item_count: usize,
}

/// A displayable debian entry for table output.
#[derive(Tabled, Serialize)]
struct DebianRow {
    #[tabled(rename = "Package")]
    name: String,
    #[tabled(rename = "Codename")]
    codename: String,
    #[tabled(rename = "Arch")]
    architecture: String,
    #[tabled(rename = "Size")]
    size: String,
    #[tabled(rename = "Modified")]
    modified: String,
}

/// Classify a folder name as "build-id", "legacy", or "other".
fn classify_folder(name: &str) -> &'static str {
    if name == "legacy" {
        "legacy"
    } else if is_buildkite_id(name) {
        "build-id"
    } else {
        "other"
    }
}

/// Check if a folder name looks like a Buildkite build ID (UUID format).
fn is_buildkite_id(name: &str) -> bool {
    // Buildkite build IDs are UUIDs: 8-4-4-4-12 hex chars
    let parts: Vec<&str> = name.split('-').collect();
    if parts.len() == 5 {
        let expected_lens = [8, 4, 4, 4, 12];
        parts
            .iter()
            .zip(expected_lens.iter())
            .all(|(part, &len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
    } else {
        false
    }
}

/// Execute the list command.
pub fn execute(
    backend: &dyn CacheBackend,
    cache_base: &str,
    folder: Option<&str>,
    debians: bool,
    format: &OutputFormat,
) -> Result<()> {
    let base = PathBuf::from(cache_base);

    match folder {
        None => list_top_level(backend, &base, format),
        Some(f) if debians => list_debians(backend, &base.join(f), format),
        Some(f) => list_folder_contents(backend, &base.join(f), format),
    }
}

/// List all top-level folders in the cache root.
fn list_top_level(backend: &dyn CacheBackend, base: &PathBuf, format: &OutputFormat) -> Result<()> {
    let entries = backend.list_dir(base)?;
    let rows: Vec<FolderRow> = entries
        .iter()
        .filter(|e| e.is_dir)
        .map(|e| {
            let item_count = backend
                .list_dir(&e.path)
                .map(|items| items.len())
                .unwrap_or(0);
            FolderRow {
                name: e.name.clone(),
                folder_type: classify_folder(&e.name).to_string(),
                modified: e.modified.format("%Y-%m-%d %H:%M:%S").to_string(),
                item_count,
            }
        })
        .collect();

    print_output(&rows, format)
}

/// List contents of a specific folder.
fn list_folder_contents(
    backend: &dyn CacheBackend,
    path: &PathBuf,
    format: &OutputFormat,
) -> Result<()> {
    if !backend.exists(path) {
        anyhow::bail!("Folder does not exist: {}", path.display());
    }

    let entries = backend.list_dir(path)?;
    let rows: Vec<FolderRow> = entries
        .iter()
        .map(|e| {
            let item_count = if e.is_dir {
                backend
                    .list_dir(&e.path)
                    .map(|items| items.len())
                    .unwrap_or(0)
            } else {
                0
            };
            FolderRow {
                name: e.name.clone(),
                folder_type: if e.is_dir { "dir" } else { "file" }.to_string(),
                modified: e.modified.format("%Y-%m-%d %H:%M:%S").to_string(),
                item_count,
            }
        })
        .collect();

    print_output(&rows, format)
}

/// List debian packages with codename/architecture awareness.
///
/// Expected folder structure under the target folder:
/// ```text
/// <folder>/debians/<codename>/<architecture>/<package>.deb
/// ```
///
/// Known codenames: bullseye, focal, noble, jammy, bookworm
/// Known architectures: amd64, arm64, all
pub fn list_debians(
    backend: &dyn CacheBackend,
    path: &PathBuf,
    format: &OutputFormat,
) -> Result<()> {
    if !backend.exists(path) {
        anyhow::bail!("Folder does not exist: {}", path.display());
    }

    let debian_entries = collect_debians(backend, path)?;

    let rows: Vec<DebianRow> = debian_entries
        .iter()
        .map(|d| DebianRow {
            name: d.name.clone(),
            codename: d.codename.clone(),
            architecture: d.architecture.clone(),
            size: format_size(d.size),
            modified: d.modified.format("%Y-%m-%d %H:%M:%S").to_string(),
        })
        .collect();

    print_output(&rows, format)
}

/// Collect debian entries by walking the expected structure.
///
/// Walks: `base_path/` looking for any of:
///   - `debians/<codename>/<arch>/*.deb`
///   - `<codename>/<arch>/*.deb`
///   - `*.deb` (flat structure, codename/arch = "unknown")
pub fn collect_debians(
    backend: &dyn CacheBackend,
    base_path: &PathBuf,
) -> Result<Vec<DebianEntry>> {
    let mut result = Vec::new();

    // Try debians/ subfolder first
    let debians_path = base_path.join("debians");
    let search_root = if backend.is_dir(&debians_path) {
        debians_path
    } else {
        base_path.clone()
    };

    // Walk codenames
    if let Ok(entries) = backend.list_dir(&search_root) {
        for entry in &entries {
            if entry.is_dir && KNOWN_CODENAMES.contains(&entry.name.as_str()) {
                collect_debs_under_codename(backend, &entry.path, &entry.name, &mut result)?;
                continue;
            }
            // Flat .deb files at root level
            if !entry.is_dir && entry.name.ends_with(".deb") {
                result.push(DebianEntry {
                    name: entry.name.clone(),
                    codename: "unknown".to_string(),
                    architecture: detect_arch_from_filename(&entry.name),
                    path: entry.path.clone(),
                    size: entry.size,
                    modified: entry.modified,
                });
            }
        }
    }

    result.sort_by(|a, b| {
        a.codename
            .cmp(&b.codename)
            .then(a.architecture.cmp(&b.architecture))
            .then(a.name.cmp(&b.name))
    });

    Ok(result)
}

fn collect_debs_under_codename(
    backend: &dyn CacheBackend,
    codename_path: &PathBuf,
    codename: &str,
    result: &mut Vec<DebianEntry>,
) -> Result<()> {
    if let Ok(entries) = backend.list_dir(codename_path) {
        for entry in &entries {
            if entry.is_dir && KNOWN_ARCHITECTURES.contains(&entry.name.as_str()) {
                collect_debs_in_dir(backend, &entry.path, codename, &entry.name, result)?;
            } else if !entry.is_dir && entry.name.ends_with(".deb") {
                // .deb directly under codename (no arch subfolder)
                result.push(DebianEntry {
                    name: entry.name.clone(),
                    codename: codename.to_string(),
                    architecture: detect_arch_from_filename(&entry.name),
                    path: entry.path.clone(),
                    size: entry.size,
                    modified: entry.modified,
                });
            }
        }
    }
    Ok(())
}

fn collect_debs_in_dir(
    backend: &dyn CacheBackend,
    dir_path: &PathBuf,
    codename: &str,
    architecture: &str,
    result: &mut Vec<DebianEntry>,
) -> Result<()> {
    if let Ok(entries) = backend.list_dir(dir_path) {
        for entry in &entries {
            if !entry.is_dir && entry.name.ends_with(".deb") {
                result.push(DebianEntry {
                    name: entry.name.clone(),
                    codename: codename.to_string(),
                    architecture: architecture.to_string(),
                    path: entry.path.clone(),
                    size: entry.size,
                    modified: entry.modified,
                });
            }
        }
    }
    Ok(())
}

/// Try to detect architecture from a .deb filename.
/// Convention: `<name>_<version>_<arch>.deb`
fn detect_arch_from_filename(name: &str) -> String {
    let without_ext = name.trim_end_matches(".deb");
    if let Some(arch) = without_ext.rsplit('_').next() {
        if KNOWN_ARCHITECTURES.contains(&arch) {
            return arch.to_string();
        }
    }
    "unknown".to_string()
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn print_output<T: Tabled + Serialize>(rows: &[T], format: &OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Text => {
            if rows.is_empty() {
                println!("(empty)");
            } else {
                let table = Table::new(rows).to_string();
                println!("{}", table);
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(rows)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_folder() {
        assert_eq!(classify_folder("legacy"), "legacy");
        assert_eq!(
            classify_folder("a1b2c3d4-e5f6-7890-abcd-ef1234567890"),
            "build-id"
        );
        assert_eq!(classify_folder("some-folder"), "other");
    }

    #[test]
    fn test_is_buildkite_id() {
        assert!(is_buildkite_id("a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
        assert!(is_buildkite_id("01234567-89ab-cdef-0123-456789abcdef"));
        assert!(!is_buildkite_id("not-a-uuid"));
        assert!(!is_buildkite_id("legacy"));
        assert!(!is_buildkite_id(""));
    }

    #[test]
    fn test_detect_arch_from_filename() {
        assert_eq!(
            detect_arch_from_filename("mina-devnet_1.0.0_amd64.deb"),
            "amd64"
        );
        assert_eq!(
            detect_arch_from_filename("mina-devnet_1.0.0_arm64.deb"),
            "arm64"
        );
        assert_eq!(
            detect_arch_from_filename("mina-devnet_1.0.0_all.deb"),
            "all"
        );
        assert_eq!(detect_arch_from_filename("mina-devnet.deb"), "unknown");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(1073741824), "1.0 GB");
    }
}
