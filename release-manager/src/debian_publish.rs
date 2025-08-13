use crate::errors::{ManagerResult, ManagerError};
use std::path::Path;
use tokio::process::Command as AsyncCommand;
use chrono::NaiveDateTime;
use chrono::Utc;


/// Configuration for Debian package publishing
#[derive(Debug, Clone)]
pub struct DebianPublishConfig {
    /// Debian package file path
    pub package_path: String,
    /// Version to publish
    pub version: String,
    /// S3 bucket for repository
    pub bucket: String,
    /// Codename (bullseye, focal, etc.)
    pub codename: String,
    /// Release channel (stable, unstable, etc.)
    pub release: String,
    /// Optional GPG signing key
    pub sign_key: Option<String>,
    /// Debug flag to enable verbose output
    pub debug: bool,
}

/// Debian package publisher using deb-s3
pub struct DebianPublisher {
    config: DebianPublishConfig,
}

impl DebianPublisher {
    /// Create a new DebianPublisher
    pub fn new(config: DebianPublishConfig) -> Self {
        Self { config }
    }

    /// Remove stale lockfile from S3 repository
    pub async fn remove_lockfile(&self) -> ManagerResult<()> {
        println!("    🔍 Checking lockfile status...");

        let lockfile_path = format!(
            "s3://{}/dists/{}/{}/binary-/lockfile",
            self.config.bucket, self.config.codename, self.config.release
        );

        // Check if lockfile exists and get its timestamp
        let mut ls_cmd = AsyncCommand::new("aws");
        ls_cmd.args(&["s3", "ls", &lockfile_path]);

        let ls_output = ls_cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to execute aws s3 ls: {}", e)))?;

        if !ls_output.status.success() {
            println!("    ℹ️  No lockfile found");
            return Ok(());
        }

        let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
        if ls_stdout.trim().is_empty() {
            println!("    ℹ️  No lockfile found");
            return Ok(());
        }

        // Parse the timestamp from ls output (format: "2023-12-01 14:30:45")
        let parts: Vec<&str> = ls_stdout.split_whitespace().collect();
        if parts.len() < 2 {
            println!("    ⚠️  Could not parse lockfile timestamp. Deleting anyway...");
            self.delete_lockfile(&lockfile_path).await?;
            return Err(ManagerError::CommandFailed(
                "Could not get lockfile timestamp from S3 bucket. Check AWS credentials.".to_string()
            ));
        }

        let date_str = format!("{} {}", parts[0], parts[1]);
        
        // Parse the timestamp using chrono
        let lockfile_time = NaiveDateTime::parse_from_str(&date_str, "%Y-%m-%d %H:%M:%S")
            .map_err(|e| ManagerError::ValidationError(format!("Failed to parse timestamp: {}", e)))?
            .and_utc();

        let now = Utc::now();
        let time_diff = now.signed_duration_since(lockfile_time).num_seconds();

        if time_diff > 300 {
            println!("    🕒 Lockfile is older than 5 minutes ({} seconds). Deleting...", time_diff);
            self.delete_lockfile(&lockfile_path).await?;
            println!("    ✅ Lockfile deleted");
        } else {
            println!("    ⏰ Lockfile is younger than 5 minutes ({} seconds). Refusing to delete.", time_diff);
            return Err(ManagerError::ValidationError(
                "Lockfile is too recent. There may be an active deb-s3 instance using it.".to_string()
            ));
        }

        Ok(())
    }

    /// Delete lockfile from S3
    async fn delete_lockfile(&self, lockfile_path: &str) -> ManagerResult<()> {
        let mut rm_cmd = AsyncCommand::new("aws");
        rm_cmd.args(&["s3", "rm", lockfile_path]);

        let rm_output = rm_cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to execute aws s3 rm: {}", e)))?;

        if !rm_output.status.success() {
            let stderr = String::from_utf8_lossy(&rm_output.stderr);
            return Err(ManagerError::CommandFailed(format!(
                "Failed to delete lockfile: {}", stderr
            )));
        }

        Ok(())
    }

    /// Publish the Debian package to S3 repository
    pub async fn publish(&self) -> ManagerResult<()> {
        self.validate_config()?;
        
        println!(" 📦 Publishing Debian package to S3 repository:");
        println!("    📁 Package: {}", self.config.package_path);
        println!("    🏷️  Version: {}", self.config.version);
        println!("    🪣 Bucket: {}", self.config.bucket);
        println!("    📋 Codename: {}", self.config.codename);
        println!("    🚀 Release: {}", self.config.release);

        // Check if package file exists
        if !Path::new(&self.config.package_path).exists() {
            return Err(ManagerError::ValidationError(format!(
                "Package file not found: {}", self.config.package_path
            )));
        }

        // Build deb-s3 upload command
        let mut cmd = AsyncCommand::new("deb-s3");
        cmd.arg("upload")
           .arg("--s3-region=us-west-2")
           .arg("--bucket")
           .arg(&self.config.bucket)
           .arg("--codename")
           .arg(&self.config.codename)
           .arg("--component")
           .arg(&self.config.release)
           .arg("--suite")
           .arg(&self.config.release)
           .arg("--preserve-versions")
           .arg("--lock")
           .arg("--fail-if-exists")
           .arg("--cache-control=max-age=120")
           .arg(&self.config.package_path);

        // Add signing if specified
        if let Some(sign_key) = &self.config.sign_key {
            cmd.arg("--sign").arg(sign_key);
        }

        println!("    🔄 Executing: deb-s3 upload...");
        if self.config.debug {
            println!("    📜 Command: {:?}", cmd);
        }

        let output = cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to execute deb-s3: {}", e)))?;



        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);

            println!("    ❌ Upload failed");
            // Check if error is due to lockfile conflict
            if stderr.contains("lockfile") || stderr.contains("locked") {
                println!("    🔒 Lockfile conflict detected. Attempting to remove stale lockfile...");
                if let Err(lockfile_err) = self.remove_lockfile().await {
                    println!("    ⚠️  Failed to remove lockfile: {}", lockfile_err);
                } else {
                    println!("    ✅ Lockfile removed. Please retry the upload.");
                }
            }

            return Err(ManagerError::CommandFailed(format!(
                "deb-s3 upload failed. Stdout: {}, Stderr: {}", stdout, stderr
            )));
        }



        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("    ✅ Upload completed successfully");
        if !stdout.is_empty() {
            println!("    📄 Output: {}", stdout.trim());
        }

        // Verify the upload
        self.verify_upload().await?;

        Ok(())
    }

    /// Verify that the package was uploaded successfully
    async fn verify_upload(&self) -> ManagerResult<()> {
        println!("    🔍 Verifying package upload...");

        // Build deb-s3 verify command
        let mut cmd = AsyncCommand::new("deb-s3");
        cmd.arg("verify")
           .arg("--bucket")
           .arg(&self.config.bucket)
           .arg("--s3-region=us-west-2")
           .arg("--codename")
           .arg(&self.config.codename)
           .arg("--component")
           .arg(&self.config.release)
           .arg("--suite")
           .arg(&self.config.release);

        let output = cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to execute deb-s3 verify: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ManagerError::CommandFailed(format!(
                "deb-s3 verify failed: {}", stderr
            )));
        }

        println!("    ✅ Package verification successful");
        Ok(())
    }

    /// Validate configuration parameters
    fn validate_config(&self) -> ManagerResult<()> {
        if self.config.package_path.is_empty() {
            return Err(ManagerError::ValidationError("Package path cannot be empty".to_string()));
        }
        if self.config.version.is_empty() {
            return Err(ManagerError::ValidationError("Version cannot be empty".to_string()));
        }
        if self.config.bucket.is_empty() {
            return Err(ManagerError::ValidationError("Bucket cannot be empty".to_string()));
        }
        if self.config.codename.is_empty() {
            return Err(ManagerError::ValidationError("Codename cannot be empty".to_string()));
        }
        if self.config.release.is_empty() {
            return Err(ManagerError::ValidationError("Release cannot be empty".to_string()));
        }
        Ok(())
    }
}

/// High-level function to publish a Debian package
pub async fn publish_debian_package(
    package_path: &str,
    version: &str,
    bucket: &str,
    codename: &str,
    release: &str,
    sign_key: Option<&str>,
    debug: bool,    
) -> ManagerResult<()> {
    let config = DebianPublishConfig {
        package_path: package_path.to_string(),
        version: version.to_string(),
        bucket: bucket.to_string(),
        codename: codename.to_string(),
        release: release.to_string(),
        sign_key: sign_key.map(|s| s.to_string()),
        debug,
    };

    let publisher = DebianPublisher::new(config);
    publisher.publish().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_config_validation() {
        let temp_file = NamedTempFile::new().unwrap();
        let config = DebianPublishConfig {
            package_path: temp_file.path().to_string_lossy().to_string(),
            version: "1.0.0".to_string(),
            bucket: "test-bucket".to_string(),
            codename: "bullseye".to_string(),
            release: "stable".to_string(),
            sign_key: None,
            debug: false,
        };

        let publisher = DebianPublisher::new(config);
        assert!(publisher.validate_config().is_ok());
    }

    #[test]
    fn test_empty_version_validation() {
        let config = DebianPublishConfig {
            package_path: "/tmp/test.deb".to_string(),
            version: "".to_string(),
            bucket: "test-bucket".to_string(),
            codename: "bullseye".to_string(),
            release: "stable".to_string(),
            sign_key: None,
            debug: false,
        };

        let publisher = DebianPublisher::new(config);
        assert!(publisher.validate_config().is_err());
    }

    #[test]
    fn test_empty_bucket_validation() {
        let config = DebianPublishConfig {
            package_path: "/tmp/test.deb".to_string(),
            version: "1.0.0".to_string(),
            bucket: "".to_string(),
            codename: "bullseye".to_string(),
            release: "stable".to_string(),
            sign_key: None,
            debug: false,
        };

        let publisher = DebianPublisher::new(config);
        assert!(publisher.validate_config().is_err());
    }
}