use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use regex::Regex;
use serde::Serialize;

use crate::cache::{CacheBackend, CacheEntry};
use crate::cli::{FolderType, OutputFormat};

#[derive(Serialize)]
struct PruneEntry {
    name: String,
    modified: String,
    action: String,
}

#[derive(Serialize)]
struct PruneResult {
    dry_run: bool,
    entries: Vec<PruneEntry>,
    total: usize,
}

/// Execute the prune command: remove cache folders based on conditions.
pub fn execute(
    backend: &dyn CacheBackend,
    cache_base: &str,
    older_than: Option<&str>,
    keep_latest_versions: Option<usize>,
    keep_latest_timestamp: Option<usize>,
    folder_type: &FolderType,
    dry_run: bool,
    format: &OutputFormat,
) -> Result<()> {
    let base = PathBuf::from(cache_base);
    let entries = backend.list_dir(&base)?;

    let folders: Vec<CacheEntry> = entries
        .into_iter()
        .filter(|e| e.is_dir && matches_folder_type(&e.name, folder_type))
        .collect();

    if folders.is_empty() {
        match format {
            OutputFormat::Text => println!("No folders matching criteria found."),
            OutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&PruneResult {
                    dry_run,
                    entries: vec![],
                    total: 0,
                })?
            ),
        }
        return Ok(());
    }

    let to_remove = determine_removals(
        &folders,
        older_than,
        keep_latest_versions,
        keep_latest_timestamp,
    )?;

    if to_remove.is_empty() {
        match format {
            OutputFormat::Text => println!("Nothing to prune."),
            OutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&PruneResult {
                    dry_run,
                    entries: vec![],
                    total: 0,
                })?
            ),
        }
        return Ok(());
    }

    let mut prune_entries = Vec::new();

    for entry in &to_remove {
        let modified_str = entry.modified.format("%Y-%m-%d %H:%M:%S").to_string();
        let action = if dry_run { "would_remove" } else { "removed" };

        if *format == OutputFormat::Text {
            if dry_run {
                println!(
                    "[DRY RUN] Would remove: {} (modified: {})",
                    entry.name, modified_str
                );
            } else {
                println!("Removing: {} (modified: {})", entry.name, modified_str);
            }
        }

        if !dry_run {
            backend
                .remove_dir_all(&entry.path)
                .with_context(|| format!("Failed to remove: {}", entry.path.display()))?;
        }

        prune_entries.push(PruneEntry {
            name: entry.name.clone(),
            modified: modified_str,
            action: action.to_string(),
        });
    }

    match format {
        OutputFormat::Text => {
            let action = if dry_run { "Would prune" } else { "Pruned" };
            println!("{} {} folder(s).", action, to_remove.len());
        }
        OutputFormat::Json => {
            let result = PruneResult {
                dry_run,
                entries: prune_entries,
                total: to_remove.len(),
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}

/// Determine which folders to remove based on the pruning criteria.
pub fn determine_removals<'a>(
    folders: &'a [CacheEntry],
    older_than: Option<&str>,
    keep_latest_versions: Option<usize>,
    keep_latest_timestamp: Option<usize>,
) -> Result<Vec<&'a CacheEntry>> {
    let mut candidates: Vec<&CacheEntry> = folders.iter().collect();

    // Filter by age
    if let Some(duration_str) = older_than {
        let duration = parse_duration(duration_str)?;
        let cutoff = Utc::now() - duration;
        candidates.retain(|e| e.modified < cutoff);
    }

    // Keep only latest N by version (sort by version descending, remove older)
    if let Some(keep_n) = keep_latest_versions {
        let mut version_sorted: Vec<&CacheEntry> = folders.iter().collect();
        version_sorted.sort_by(|a, b| compare_versions(&b.name, &a.name));
        let to_keep: Vec<&str> = version_sorted
            .iter()
            .take(keep_n)
            .map(|e| e.name.as_str())
            .collect();
        candidates.retain(|e| !to_keep.contains(&e.name.as_str()));
    }

    // Keep only latest N by timestamp
    if let Some(keep_n) = keep_latest_timestamp {
        let mut time_sorted: Vec<&CacheEntry> = folders.iter().collect();
        time_sorted.sort_by(|a, b| b.modified.cmp(&a.modified));
        let to_keep: Vec<&str> = time_sorted
            .iter()
            .take(keep_n)
            .map(|e| e.name.as_str())
            .collect();
        candidates.retain(|e| !to_keep.contains(&e.name.as_str()));
    }

    // Deduplicate
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    candidates.dedup_by(|a, b| a.name == b.name);

    Ok(candidates)
}

/// Parse a human-readable duration string like "30d", "12h", "2w".
pub fn parse_duration(s: &str) -> Result<Duration> {
    let re = Regex::new(r"^(\d+)\s*([dhwm])$")?;
    let caps = re.captures(s).with_context(|| {
        format!(
            "Invalid duration format: '{}'. Use e.g., '30d', '12h', '2w', '3m'",
            s
        )
    })?;
    let value: i64 = caps[1].parse()?;
    let unit = &caps[2];
    match unit {
        "h" => Ok(Duration::hours(value)),
        "d" => Ok(Duration::days(value)),
        "w" => Ok(Duration::weeks(value)),
        "m" => Ok(Duration::days(value * 30)), // approximate months
        _ => anyhow::bail!("Unsupported duration unit: {}", unit),
    }
}

/// Compare two strings as version numbers.
/// Extracts numeric parts and compares them; falls back to lexicographic.
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    static DIGIT_RE: OnceLock<Regex> = OnceLock::new();
    let extract = |s: &str| -> Vec<u64> {
        let re = DIGIT_RE.get_or_init(|| Regex::new(r"\d+").expect("static regex"));
        re.find_iter(s)
            .filter_map(|m| m.as_str().parse().ok())
            .collect()
    };
    let va = extract(a);
    let vb = extract(b);
    va.cmp(&vb).then_with(|| a.cmp(b))
}

fn matches_folder_type(name: &str, folder_type: &FolderType) -> bool {
    match folder_type {
        FolderType::Legacy => name == "legacy",
        FolderType::BuildId => is_uuid_like(name),
        FolderType::All => true,
    }
}

fn is_uuid_like(name: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("30d").unwrap(), Duration::days(30));
        assert_eq!(parse_duration("12h").unwrap(), Duration::hours(12));
        assert_eq!(parse_duration("2w").unwrap(), Duration::weeks(2));
        assert_eq!(parse_duration("3m").unwrap(), Duration::days(90));
        assert!(parse_duration("invalid").is_err());
        assert!(parse_duration("30x").is_err());
    }

    #[test]
    fn test_compare_versions() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.1", "1.0.0"), Ordering::Greater);
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "2.0.0"), Ordering::Less);
    }

    #[test]
    fn test_matches_folder_type() {
        assert!(matches_folder_type("legacy", &FolderType::Legacy));
        assert!(!matches_folder_type("legacy", &FolderType::BuildId));
        assert!(matches_folder_type("legacy", &FolderType::All));

        let uuid = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
        assert!(matches_folder_type(uuid, &FolderType::BuildId));
        assert!(!matches_folder_type(uuid, &FolderType::Legacy));
        assert!(matches_folder_type(uuid, &FolderType::All));
    }
}
