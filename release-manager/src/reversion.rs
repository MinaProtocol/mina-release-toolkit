use crate::errors::{ManagerResult, ManagerError};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tokio::process::Command as AsyncCommand;
use tempfile::TempDir;

/// Configuration for Debian package reversion
#[derive(Debug, Clone)]
pub struct ReversionConfig {
    /// Path to the source .deb file
    pub deb_path: PathBuf,
    /// Original package name
    pub package_name: String,
    /// Source version to reversion from
    pub source_version: String,
    /// New version to reversion to
    pub new_version: String,
    /// Current suite (e.g., "unstable")
    pub suite: String,
    /// New suite (e.g., "stable")
    pub new_suite: String,
    /// New package name (if different from original)
    pub new_name: Option<String>,
}

/// Debian package reversion functionality
pub struct DebianReversioner {
    config: ReversionConfig,
    temp_dir: TempDir,
}

impl DebianReversioner {
    /// Create a new DebianReversioner with the given configuration
    pub fn new(config: ReversionConfig) -> ManagerResult<Self> {
        let temp_dir = TempDir::new()
            .map_err(|e| ManagerError::IoError(io::Error::new(io::ErrorKind::Other, format!("Failed to create temp directory: {}", e))))?;
        
        Ok(Self {
            config,
            temp_dir,
        })
    }

    /// Perform the complete reversion process
    pub async fn reversion(&self) -> ManagerResult<PathBuf> {
        self.validate_inputs()?;
        
        println!(" 🔄 Reversioning Debian package:");
        println!("    📦 Source: {} v{}", self.config.package_name, self.config.source_version);
        println!("    🎯 Target: {} v{}", 
                self.config.new_name.as_ref().unwrap_or(&self.config.package_name), 
                self.config.new_version);
        println!("    📂 Suite: {} → {}", self.config.suite, self.config.new_suite);

        // Extract the original package
        let extract_dir = self.extract_package().await?;
        
        // Modify package metadata
        self.modify_control_files(&extract_dir).await?;
        
        // Rebuild the package with new version
        let new_deb_path = self.rebuild_package(&extract_dir).await?;
        
        println!(" ✅ Reversion completed: {}", new_deb_path.display());
        
        Ok(new_deb_path)
    }

    /// Validate input parameters
    fn validate_inputs(&self) -> ManagerResult<()> {
        if !self.config.deb_path.exists() {
            return Err(ManagerError::IoError(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Source .deb file does not exist: {}", self.config.deb_path.display())
            )));
        }

        if self.config.source_version.is_empty() {
            return Err(ManagerError::ValidationError("Source version cannot be empty".to_string()));
        }

        if self.config.new_version.is_empty() {
            return Err(ManagerError::ValidationError("New version cannot be empty".to_string()));
        }

        if self.config.package_name.is_empty() {
            return Err(ManagerError::ValidationError("Package name cannot be empty".to_string()));
        }

        Ok(())
    }

    /// Extract the Debian package using dpkg-deb
    async fn extract_package(&self) -> ManagerResult<PathBuf> {
        let extract_dir = self.temp_dir.path().join("extracted");
        fs::create_dir_all(&extract_dir)?;

        println!("    📤 Extracting package: {}", self.config.deb_path.display());
        
        let mut cmd = AsyncCommand::new("dpkg-deb");
        cmd.arg("-R")
           .arg(&self.config.deb_path)
           .arg(&extract_dir);

        let output = cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to execute dpkg-deb: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ManagerError::CommandFailed(format!(
                "dpkg-deb extraction failed: {}", stderr
            )));
        }

        Ok(extract_dir)
    }

    /// Modify control files with new version and metadata
    async fn modify_control_files(&self, extract_dir: &Path) -> ManagerResult<()> {
        let control_file = extract_dir.join("DEBIAN").join("control");
        
        if !control_file.exists() {
            return Err(ManagerError::IoError(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Control file not found: {}", control_file.display())
            )));
        }

        println!("    ✏️  Modifying control file: {}", control_file.display());
        
        // Read the control file
        let control_content = fs::read_to_string(&control_file)?;

        // Modify the control file content
        let new_content = self.update_control_content(&control_content)?;

        // Write the modified control file
        fs::write(&control_file, new_content)?;

        // Update changelog if it exists
        self.update_changelog(extract_dir).await?;

        Ok(())
    }

    /// Update the content of the control file
    fn update_control_content(&self, content: &str) -> ManagerResult<String> {
        let mut result = content.to_string();
        let mut modified = false;

        // Update package name if specified
        if let Some(new_name) = &self.config.new_name {
            let package_pattern = format!("Package: {}", self.config.package_name);
            let new_package_line = format!("Package: {}", new_name);
            if result.contains(&package_pattern) {
                result = result.replace(&package_pattern, &new_package_line);
                modified = true;
            }
        }

        // Update version - be more careful with version replacement
        let version_pattern = format!("Version: ");
        if let Some(version_line_start) = result.find(&version_pattern) {
            if let Some(version_line_end) = result[version_line_start..].find('\n') {
                let version_line_end = version_line_start + version_line_end;
                let version_line = &result[version_line_start..version_line_end];
                
                if version_line.contains(&self.config.source_version) {
                    let new_version_line = version_line.replace(&self.config.source_version, &self.config.new_version);
                    result.replace_range(version_line_start..version_line_end, &new_version_line);
                    modified = true;
                }
            }
        }

        // Update distribution/suite in control file if present
        let distribution_pattern = "Distribution: ";
        if let Some(dist_start) = result.find(distribution_pattern) {
            if let Some(dist_end) = result[dist_start..].find('\n') {
                let dist_end = dist_start + dist_end;
                let new_dist_line = format!("Distribution: {}", self.config.new_suite);
                result.replace_range(dist_start..dist_end, &new_dist_line);
                modified = true;
            }
        }

        if !modified {
            println!("    ⚠️  Warning: No modifications made to control file");
        }

        // Ensure the control file ends with a newline (required by Debian format)
        if !result.ends_with('\n') {
            result.push('\n');
        }

        // Also ensure there are no trailing spaces that might cause issues
        let lines: Vec<String> = result.lines().map(|line| line.trim_end().to_string()).collect();
        result = lines.join("\n");
        if !result.ends_with('\n') {
            result.push('\n');
        }

        Ok(result)
    }

    /// Update changelog file if it exists
    async fn update_changelog(&self, extract_dir: &Path) -> ManagerResult<()> {
        let changelog_paths = [
            extract_dir.join("usr").join("share").join("doc").join(&self.config.package_name).join("changelog.Debian.gz"),
            extract_dir.join("usr").join("share").join("doc").join(&self.config.package_name).join("changelog.gz"),
        ];

        for changelog_path in &changelog_paths {
            if changelog_path.exists() {
                println!("    📝 Updating changelog: {}", changelog_path.display());
                // For simplicity, we'll create a new changelog entry
                // In a full implementation, you might want to decompress, modify, and recompress
                self.create_changelog_entry(extract_dir).await?;
                break;
            }
        }

        Ok(())
    }

    /// Create a new changelog entry
    async fn create_changelog_entry(&self, extract_dir: &Path) -> ManagerResult<()> {
        let package_name = self.config.new_name.as_ref().unwrap_or(&self.config.package_name);
        let doc_dir = extract_dir.join("usr").join("share").join("doc").join(package_name);
        
        if let Err(e) = fs::create_dir_all(&doc_dir) {
            println!("    ⚠️  Warning: Could not create doc directory: {}", e);
            return Ok(());
        }

        let changelog_content = format!(
            "{} ({}) {}; urgency=medium\n\n  * Reversion from {} to {}\n  * Automated reversion by release-manager\n\n -- Release Manager <release@minaprotocol.com>  {}\n\n",
            package_name,
            self.config.new_version,
            self.config.new_suite,
            self.config.source_version,
            self.config.new_version,
            chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S +0000")
        );

        let changelog_file = doc_dir.join("changelog.Debian");
        if let Err(e) = fs::write(&changelog_file, changelog_content) {
            println!("    ⚠️  Warning: Could not write changelog: {}", e);
        }

        // Compress the changelog
        let mut cmd = AsyncCommand::new("gzip");
        cmd.arg("-f").arg(&changelog_file);
        
        if let Err(e) = cmd.output().await {
            println!("    ⚠️  Warning: Could not compress changelog: {}", e);
        }

        Ok(())
    }

    /// Rebuild the package with new metadata
    async fn rebuild_package(&self, extract_dir: &Path) -> ManagerResult<PathBuf> {
        let new_package_name = self.config.new_name.as_ref().unwrap_or(&self.config.package_name);
        let new_deb_filename = format!("{}_{}.deb", new_package_name, self.config.new_version);
        let new_deb_path = self.config.deb_path.parent()
            .unwrap_or_else(|| Path::new("."))
            .join(&new_deb_filename);

        // Remove existing file if it exists
        if new_deb_path.exists() {
            fs::remove_file(&new_deb_path)?;
        }

        println!("    📦 Building new package: {}", new_deb_path.display());

        let mut cmd = AsyncCommand::new("dpkg-deb");
        cmd.arg("--build")
           .arg(extract_dir)
           .arg(&new_deb_path);

        let output = cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to execute dpkg-deb build: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ManagerError::CommandFailed(format!(
                "dpkg-deb build failed: {}", stderr
            )));
        }

        // Validate the new package was created
        if !new_deb_path.exists() {
            return Err(ManagerError::IoError(io::Error::new(
                io::ErrorKind::NotFound,
                format!("New .deb package was not created: {}", new_deb_path.display())
            )));
        }

        Ok(new_deb_path)
    }
}

/// High-level function to perform debian package reversion
pub async fn reversion_debian_package(
    deb_path: &Path,
    package_name: &str,
    source_version: &str,
    new_version: &str,
    suite: &str,
    new_suite: &str,
    new_name: Option<&str>,
) -> ManagerResult<PathBuf> {
    let config = ReversionConfig {
        deb_path: deb_path.to_path_buf(),
        package_name: package_name.to_string(),
        source_version: source_version.to_string(),
        new_version: new_version.to_string(),
        suite: suite.to_string(),
        new_suite: new_suite.to_string(),
        new_name: new_name.map(|s| s.to_string()),
    };

    let reversioner = DebianReversioner::new(config)?;
    reversioner.reversion().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_reversion_config_validation() {
        let temp_file = NamedTempFile::new().unwrap();
        let config = ReversionConfig {
            deb_path: temp_file.path().to_path_buf(),
            package_name: "test-package".to_string(),
            source_version: "1.0.0".to_string(),
            new_version: "1.0.1".to_string(),
            suite: "unstable".to_string(),
            new_suite: "stable".to_string(),
            new_name: None,
        };

        let reversioner = DebianReversioner::new(config).unwrap();
        assert!(reversioner.validate_inputs().is_ok());
    }

    #[test]
    fn test_control_content_update() {
        let temp_file = NamedTempFile::new().unwrap();
        let config = ReversionConfig {
            deb_path: temp_file.path().to_path_buf(),
            package_name: "test-package".to_string(),
            source_version: "1.0.0".to_string(),
            new_version: "1.0.1".to_string(),
            suite: "unstable".to_string(),
            new_suite: "stable".to_string(),
            new_name: Some("new-package".to_string()),
        };

        let reversioner = DebianReversioner::new(config).unwrap();
        
        let control_content = r#"Package: test-package
Version: 1.0.0-1
Architecture: amd64
Maintainer: Test <test@example.com>
Description: Test package
 Multi-line description
 with more details
"#;

        let updated_content = reversioner.update_control_content(control_content).unwrap();
        
        assert!(updated_content.contains("Package: new-package"));
        assert!(updated_content.contains("Version: 1.0.1-1"));
        assert!(updated_content.ends_with("\n"), "Control file should end with newline");
        assert!(updated_content.contains("Multi-line description"), "Multi-line fields should be preserved");
    }

    #[test]
    fn test_invalid_deb_path() {
        let config = ReversionConfig {
            deb_path: PathBuf::from("/nonexistent/path.deb"),
            package_name: "test-package".to_string(),
            source_version: "1.0.0".to_string(),
            new_version: "1.0.1".to_string(),
            suite: "unstable".to_string(),
            new_suite: "stable".to_string(),
            new_name: None,
        };

        let reversioner = DebianReversioner::new(config).unwrap();
        assert!(reversioner.validate_inputs().is_err());
    }
}