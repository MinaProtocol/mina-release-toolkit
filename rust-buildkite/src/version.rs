//! Schema version tracking
//!
//! This module tracks the Buildkite pipeline schema version from which
//! these Rust bindings were generated.

/// The commit SHA of the Buildkite pipeline-schema repository
pub const SCHEMA_COMMIT: &str = "22ce58a7a20724aa7be9b9f004f31098d041bde4";

/// The date of the schema commit (ISO 8601 format)
pub const SCHEMA_DATE: &str = "2025-12-24";

/// The Buildkite pipeline-schema repository URL
pub const SCHEMA_REPO: &str = "https://github.com/buildkite/pipeline-schema";

/// The raw URL for the schema JSON file at the tracked commit
pub const SCHEMA_URL: &str = "https://raw.githubusercontent.com/buildkite/pipeline-schema/22ce58a7a20724aa7be9b9f004f31098d041bde4/schema.json";

/// Returns schema version information as a formatted string
pub fn schema_info() -> String {
    format!(
        "Buildkite Pipeline Schema\n  Commit: {}\n  Date: {}\n  Repository: {}",
        SCHEMA_COMMIT, SCHEMA_DATE, SCHEMA_REPO
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_commit_is_valid_sha() {
        assert_eq!(SCHEMA_COMMIT.len(), 40);
        assert!(SCHEMA_COMMIT.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_schema_info() {
        let info = schema_info();
        assert!(info.contains(SCHEMA_COMMIT));
        assert!(info.contains(SCHEMA_DATE));
    }
}
