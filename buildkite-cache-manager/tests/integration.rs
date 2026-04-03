use chrono::{Duration, Utc};

use buildkite_cache_manager::cache::CacheBackend;
use buildkite_cache_manager::cli::{FolderType, OutputFormat};
use buildkite_cache_manager::commands::{list, prune, read, write};
use buildkite_cache_manager::mock::MockBackend;

const TEXT: &OutputFormat = &OutputFormat::Text;

/// Helper to create a mock cache with typical structure.
fn setup_mock_cache() -> MockBackend {
    let mock = MockBackend::new();
    let now = Utc::now();
    let old = now - Duration::days(60);
    let recent = now - Duration::days(5);

    // Cache root
    mock.add_dir("/cache", now);

    // Build ID folders
    let build1 = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
    let build2 = "b2c3d4e5-f6a7-8901-bcde-f12345678901";
    let build3 = "c3d4e5f6-a7b8-9012-cdef-123456789012";

    mock.add_dir(&format!("/cache/{}", build1), old);
    mock.add_dir(&format!("/cache/{}", build2), recent);
    mock.add_dir(&format!("/cache/{}", build3), now);

    // Add debians under build2
    let debs_base = format!("/cache/{}/debians", build2);
    mock.add_dir(&debs_base, recent);
    mock.add_dir(&format!("{}/noble", debs_base), recent);
    mock.add_dir(&format!("{}/noble/amd64", debs_base), recent);
    mock.add_dir(&format!("{}/noble/arm64", debs_base), recent);
    mock.add_dir(&format!("{}/focal", debs_base), recent);
    mock.add_dir(&format!("{}/focal/amd64", debs_base), recent);

    mock.add_file(
        &format!("{}/noble/amd64/mina-devnet_1.0.0_amd64.deb", debs_base),
        1024 * 1024,
        recent,
    );
    mock.add_file(
        &format!("{}/noble/amd64/mina-mainnet_1.0.0_amd64.deb", debs_base),
        2 * 1024 * 1024,
        recent,
    );
    mock.add_file(
        &format!("{}/noble/arm64/mina-devnet_1.0.0_arm64.deb", debs_base),
        1024 * 1024,
        recent,
    );
    mock.add_file(
        &format!("{}/focal/amd64/mina-devnet_1.0.0_amd64.deb", debs_base),
        512 * 1024,
        recent,
    );

    // Legacy folder
    mock.add_dir("/cache/legacy", old);
    mock.add_file("/cache/legacy/old-artifact.tar.gz", 4096, old);

    mock
}

// =============================================================================
// List tests
// =============================================================================

#[test]
fn test_list_top_level() {
    let mock = setup_mock_cache();
    let result = list::execute(&mock, "/cache", None, false, TEXT);
    assert!(result.is_ok());
}

#[test]
fn test_list_nonexistent_folder() {
    let mock = setup_mock_cache();
    let result = list::execute(&mock, "/cache", Some("nonexistent"), false, TEXT);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("does not exist"));
}

#[test]
fn test_list_debians() {
    let mock = setup_mock_cache();
    let build2 = "b2c3d4e5-f6a7-8901-bcde-f12345678901";
    let result = list::execute(&mock, "/cache", Some(build2), true, &OutputFormat::Json);
    assert!(result.is_ok());
}

#[test]
fn test_list_debians_collects_correct_entries() {
    let mock = setup_mock_cache();
    let build2 = "b2c3d4e5-f6a7-8901-bcde-f12345678901";
    let path = std::path::PathBuf::from(format!("/cache/{}", build2));

    let debians = list::collect_debians(&mock, &path).unwrap();

    assert_eq!(debians.len(), 4);

    let codenames: Vec<&str> = debians.iter().map(|d| d.codename.as_str()).collect();
    assert!(codenames.contains(&"noble"));
    assert!(codenames.contains(&"focal"));

    let archs: Vec<&str> = debians.iter().map(|d| d.architecture.as_str()).collect();
    assert!(archs.contains(&"amd64"));
    assert!(archs.contains(&"arm64"));

    assert!(debians.iter().all(|d| d.name.ends_with(".deb")));
}

#[test]
fn test_list_debians_flat_structure() {
    let mock = MockBackend::new();
    let now = Utc::now();

    mock.add_dir("/cache", now);
    mock.add_dir("/cache/build1", now);
    mock.add_file("/cache/build1/mina-devnet_1.0.0_amd64.deb", 1024, now);

    let path = std::path::PathBuf::from("/cache/build1");
    let debians = list::collect_debians(&mock, &path).unwrap();

    assert_eq!(debians.len(), 1);
    assert_eq!(debians[0].codename, "unknown");
    assert_eq!(debians[0].architecture, "amd64");
}

// =============================================================================
// Prune tests
// =============================================================================

#[test]
fn test_prune_older_than() {
    let mock = setup_mock_cache();
    let build1 = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";

    let result = prune::execute(
        &mock,
        "/cache",
        Some("30d"),
        None,
        None,
        &FolderType::BuildId,
        false,
        TEXT,
    );
    assert!(result.is_ok());
    assert!(!mock.path_exists(&format!("/cache/{}", build1)));
    assert!(mock.path_exists("/cache/b2c3d4e5-f6a7-8901-bcde-f12345678901"));
    assert!(mock.path_exists("/cache/c3d4e5f6-a7b8-9012-cdef-123456789012"));
}

#[test]
fn test_prune_dry_run() {
    let mock = setup_mock_cache();
    let build1 = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";

    let result = prune::execute(
        &mock,
        "/cache",
        Some("30d"),
        None,
        None,
        &FolderType::BuildId,
        true,
        TEXT,
    );
    assert!(result.is_ok());
    assert!(mock.path_exists(&format!("/cache/{}", build1)));
}

#[test]
fn test_prune_keep_latest_timestamp() {
    let mock = setup_mock_cache();

    let result = prune::execute(
        &mock,
        "/cache",
        None,
        None,
        Some(1),
        &FolderType::BuildId,
        false,
        TEXT,
    );
    assert!(result.is_ok());
    assert!(!mock.path_exists("/cache/a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
    assert!(!mock.path_exists("/cache/b2c3d4e5-f6a7-8901-bcde-f12345678901"));
    assert!(mock.path_exists("/cache/c3d4e5f6-a7b8-9012-cdef-123456789012"));
}

#[test]
fn test_prune_legacy_only() {
    let mock = setup_mock_cache();

    let result = prune::execute(
        &mock,
        "/cache",
        Some("30d"),
        None,
        None,
        &FolderType::Legacy,
        false,
        TEXT,
    );
    assert!(result.is_ok());
    assert!(!mock.path_exists("/cache/legacy"));
    assert!(mock.path_exists("/cache/a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
}

#[test]
fn test_prune_nothing_to_prune() {
    let mock = setup_mock_cache();

    let result = prune::execute(
        &mock,
        "/cache",
        Some("90d"),
        None,
        None,
        &FolderType::BuildId,
        false,
        TEXT,
    );
    assert!(result.is_ok());
    assert!(mock.path_exists("/cache/a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
    assert!(mock.path_exists("/cache/b2c3d4e5-f6a7-8901-bcde-f12345678901"));
    assert!(mock.path_exists("/cache/c3d4e5f6-a7b8-9012-cdef-123456789012"));
}

#[test]
fn test_prune_keep_latest_versions() {
    let mock = MockBackend::new();
    let now = Utc::now();

    mock.add_dir("/cache", now);
    mock.add_dir("/cache/v1.0.0", now - Duration::days(30));
    mock.add_dir("/cache/v1.1.0", now - Duration::days(20));
    mock.add_dir("/cache/v2.0.0", now - Duration::days(10));
    mock.add_dir("/cache/v2.1.0", now);

    let result = prune::execute(
        &mock,
        "/cache",
        None,
        Some(2),
        None,
        &FolderType::All,
        false,
        TEXT,
    );
    assert!(result.is_ok());
    assert!(!mock.path_exists("/cache/v1.0.0"));
    assert!(!mock.path_exists("/cache/v1.1.0"));
    assert!(mock.path_exists("/cache/v2.0.0"));
    assert!(mock.path_exists("/cache/v2.1.0"));
}

#[test]
fn test_parse_duration() {
    assert!(prune::parse_duration("30d").is_ok());
    assert!(prune::parse_duration("12h").is_ok());
    assert!(prune::parse_duration("2w").is_ok());
    assert!(prune::parse_duration("3m").is_ok());
    assert!(prune::parse_duration("bad").is_err());
}

// =============================================================================
// Read/Write tests
// =============================================================================

#[test]
fn test_write_and_read() {
    let mock = MockBackend::new();
    let now = Utc::now();

    mock.add_dir("/local", now);
    mock.add_file("/local/artifact.tar.gz", 2048, now);
    mock.add_dir("/cache", now);

    let result = write::execute(
        &mock,
        "/cache",
        "test-build-id",
        "/local/artifact.tar.gz",
        "artifacts/",
        false,
        TEXT,
    );
    assert!(result.is_ok());
    assert!(mock.path_exists("/cache/test-build-id/artifacts/artifact.tar.gz"));

    mock.create_dir_all(std::path::Path::new("/output"))
        .unwrap();
    let result = read::execute(
        &mock,
        "/cache",
        "test-build-id",
        "artifacts/artifact.tar.gz",
        "/output",
        false,
        false,
        TEXT,
    );
    assert!(result.is_ok());
}

#[test]
fn test_read_skip_dirs_create_fails() {
    let mock = MockBackend::new();
    let now = Utc::now();

    mock.add_dir("/cache", now);
    mock.add_dir("/cache/build", now);
    mock.add_file("/cache/build/file.txt", 100, now);

    let result = read::execute(
        &mock,
        "/cache",
        "build",
        "file.txt",
        "/nonexistent",
        false,
        true,
        TEXT,
    );
    assert!(result.is_err());
}

// =============================================================================
// Determine removals (unit tests for prune logic)
// =============================================================================

#[test]
fn test_determine_removals_combined_criteria() {
    use buildkite_cache_manager::cache::CacheEntry;

    let now = Utc::now();
    let folders = vec![
        CacheEntry {
            name: "old-build".to_string(),
            path: "/cache/old-build".into(),
            is_dir: true,
            size: 0,
            modified: now - Duration::days(45),
        },
        CacheEntry {
            name: "recent-build".to_string(),
            path: "/cache/recent-build".into(),
            is_dir: true,
            size: 0,
            modified: now - Duration::days(5),
        },
        CacheEntry {
            name: "newest-build".to_string(),
            path: "/cache/newest-build".into(),
            is_dir: true,
            size: 0,
            modified: now,
        },
    ];

    let removals = prune::determine_removals(&folders, Some("30d"), None, Some(1)).unwrap();

    assert_eq!(removals.len(), 1);
    assert_eq!(removals[0].name, "old-build");
}
