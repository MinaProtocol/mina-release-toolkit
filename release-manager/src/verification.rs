use crate::errors::{ManagerResult, ManagerError};
use tokio::process::Command as AsyncCommand;

/// Configuration for Debian package verification
#[derive(Debug, Clone)]
pub struct DebianVerifyConfig {
    /// Package name to verify
    pub package: String,
    /// Version to verify
    pub version: String,
    /// Repository URL
    pub repo: String,
    /// Codename (bullseye, focal, etc.)
    pub codename: String,
    /// Channel (stable, unstable, etc.)
    pub channel: String,
    /// Whether the repository is signed
    pub signed: bool,
}

/// Configuration for Docker image verification
#[derive(Debug, Clone)]
pub struct DockerVerifyConfig {
    /// Package/image name
    pub package: String,
    /// Version to verify
    pub version: String,
    /// Docker repository
    pub repo: String,
    /// Codename
    pub codename: String,
    /// Suffix (e.g., "-devnet")
    pub suffix: String,
}

/// Debian package verifier
pub struct DebianVerifier {
    config: DebianVerifyConfig,
}

/// Docker image verifier
pub struct DockerVerifier {
    config: DockerVerifyConfig,
}

impl DebianVerifier {
    /// Create a new DebianVerifier
    pub fn new(config: DebianVerifyConfig) -> Self {
        Self { config }
    }

    /// Verify Debian package installation and functionality
    pub async fn verify(&self) -> ManagerResult<()> {
        self.validate_config()?;

        println!(" 🔍 Verifying Debian package:");
        println!("    📦 Package: {}", self.config.package);
        println!("    🏷️  Version: {}", self.config.version);
        println!("    🌐 Repository: {}", self.config.repo);
        println!("    📋 Codename: {}", self.config.codename);
        println!("    🚀 Channel: {}", self.config.channel);

        // Determine the Docker image to use for testing
        let docker_image = self.get_test_docker_image();
        
        // Create a Docker container for testing
        self.run_verification_in_docker(&docker_image).await?;

        println!("    ✅ Debian package verification successful");
        Ok(())
    }

    /// Run verification inside a Docker container
    async fn run_verification_in_docker(&self, docker_image: &str) -> ManagerResult<()> {
        println!("    🐳 Starting verification in Docker container: {}", docker_image);

        // Build the verification script
        let verification_script = self.build_verification_script();
        
        println!("    📜 Verification script:\n{}", verification_script);

        // Run the script in Docker
        let mut cmd = AsyncCommand::new("docker");
        cmd.arg("run")
           .arg("--rm")
           .arg("-i")
           .arg(docker_image)
           .arg("bash")
           .arg("-c")
           .arg(&verification_script);

        let output = cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to run Docker verification: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(ManagerError::CommandFailed(format!(
                "Docker verification failed. Stdout: {}, Stderr: {}", stdout, stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            println!("    📄 Verification output: {}", stdout.trim());
        }

        Ok(())
    }

    /// Build the verification script to run inside Docker
    fn build_verification_script(&self) -> String {
        let mut script = Vec::new();
        
        // Update package lists
        script.push("apt-get update".to_string());
        
        // Add repository if signed
        if self.config.signed {
            script.push("apt-get install -y curl gnupg2".to_string());
            script.push(format!(
                "curl -fsSL https://{}/keys/minaprotocol.asc | apt-key add -",
                self.config.repo
            ));
        }
        
        // Add repository
        script.push(format!(
            "echo 'deb [trusted=yes] https://{} {} {}' | tee /etc/apt/sources.list.d/mina.list",
            self.config.repo, self.config.codename, self.config.channel
        ));
        
        // Update package lists again
        script.push("apt-get update".to_string());
        
        // Install the package
        script.push(format!(
            "apt-get install -y {}={}",
            self.config.package, self.config.version
        ));
        
        // Run package-specific tests
        let test_commands = self.get_test_commands();
        script.extend(test_commands);
        
        script.join(" && ")
    }

    /// Get test commands based on package type
    fn get_test_commands(&self) -> Vec<String> {
        match self.config.package.as_str() {
            pkg if pkg.starts_with("mina-archive") => vec![
                "mina-archive --version".to_string(),
                "mina-archive --help".to_string(),
            ],
            "mina-logproc" => vec![
                "echo 'Skipped execution for mina-logproc'".to_string(),
            ],
            pkg if pkg.starts_with("mina-rosetta") => vec![
                "mina --version".to_string(),
                "mina-archive --version".to_string(),
                "mina-rosetta --help".to_string(),
            ],
            pkg if pkg.starts_with("mina-") => vec![
                "mina --version".to_string(),
                "mina --help".to_string(),
            ],
            _ => vec![
                format!("{} --version", self.config.package),
                format!("{} --help", self.config.package),
            ],
        }
    }

    /// Get the appropriate Docker image for testing
    fn get_test_docker_image(&self) -> String {
        match self.config.codename.as_str() {
            "bullseye" => "debian:bullseye",
            "focal" => "ubuntu:20.04",
            "jammy" => "ubuntu:22.04",
            _ => "debian:bullseye", // Default fallback
        }.to_string()
    }

    /// Validate configuration parameters
    fn validate_config(&self) -> ManagerResult<()> {
        if self.config.package.is_empty() {
            return Err(ManagerError::ValidationError("Package cannot be empty".to_string()));
        }
        if self.config.version.is_empty() {
            return Err(ManagerError::ValidationError("Version cannot be empty".to_string()));
        }
        if self.config.repo.is_empty() {
            return Err(ManagerError::ValidationError("Repository cannot be empty".to_string()));
        }
        if self.config.codename.is_empty() {
            return Err(ManagerError::ValidationError("Codename cannot be empty".to_string()));
        }
        if self.config.channel.is_empty() {
            return Err(ManagerError::ValidationError("Channel cannot be empty".to_string()));
        }
        Ok(())
    }
}

impl DockerVerifier {
    /// Create a new DockerVerifier
    pub fn new(config: DockerVerifyConfig) -> Self {
        Self { config }
    }

    /// Verify Docker image functionality
    pub async fn verify(&self) -> ManagerResult<()> {
        self.validate_config()?;

        let docker_image = format!(
            "{}:{}-{}{}",
            self.get_full_image_name(),
            self.config.version,
            self.config.codename,
            self.config.suffix
        );

        println!(" 🐋 Verifying Docker image:");
        println!("    📦 Package: {}", self.config.package);
        println!("    🏷️  Version: {}", self.config.version);
        println!("    🖼️  Image: {}", docker_image);

        // Pull the Docker image
        self.pull_image(&docker_image).await?;

        // Test the applications in the image
        self.test_applications(&docker_image).await?;

        println!("    ✅ Docker image verification successful");
        Ok(())
    }

    /// Pull the Docker image
    async fn pull_image(&self, image: &str) -> ManagerResult<()> {
        println!("    📥 Pulling Docker image: {}", image);

        let mut cmd = AsyncCommand::new("docker");
        cmd.arg("pull").arg(image);

        let output = cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to pull Docker image: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ManagerError::CommandFailed(format!(
                "Docker pull failed for {}: {}", image, stderr
            )));
        }

        Ok(())
    }

    /// Test applications in the Docker image
    async fn test_applications(&self, image: &str) -> ManagerResult<()> {
        let apps = self.get_applications();
        let commands = vec!["--version", "--help"];

        for app in &apps {
            for command in &commands {
                println!("    🧪 Testing {} {} in {}", app, command, image);
                
                let mut cmd = AsyncCommand::new("docker");
                cmd.arg("run")
                   .arg("--entrypoint")
                   .arg(app)
                   .arg("--rm")
                   .arg(image)
                   .arg(command);

                let output = cmd.output().await
                    .map_err(|e| ManagerError::CommandFailed(format!("Failed to test {} {}: {}", app, command, e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(ManagerError::CommandFailed(format!(
                        "Command {} {} failed in {}: {}", app, command, image, stderr
                    )));
                }
            }
        }

        Ok(())
    }

    /// Get applications to test based on package type
    fn get_applications(&self) -> Vec<String> {
        match self.config.package.as_str() {
            "mina-archive" => vec!["mina-archive".to_string()],
            "mina-logproc" => {
                println!("    ⏭️  Skipped execution for mina-logproc");
                vec![]
            },
            pkg if pkg.starts_with("mina-rosetta") => vec![
                "mina".to_string(),
                "mina-archive".to_string(),
                "mina-rosetta".to_string(),
            ],
            pkg if pkg.starts_with("mina-") => vec!["mina".to_string()],
            _ => vec![self.config.package.clone()],
        }
    }

    /// Get the full Docker image name
    fn get_full_image_name(&self) -> String {
        format!("{}/{}", self.config.repo, self.config.package)
    }

    /// Validate configuration parameters
    fn validate_config(&self) -> ManagerResult<()> {
        if self.config.package.is_empty() {
            return Err(ManagerError::ValidationError("Package cannot be empty".to_string()));
        }
        if self.config.version.is_empty() {
            return Err(ManagerError::ValidationError("Version cannot be empty".to_string()));
        }
        if self.config.repo.is_empty() {
            return Err(ManagerError::ValidationError("Repository cannot be empty".to_string()));
        }
        if self.config.codename.is_empty() {
            return Err(ManagerError::ValidationError("Codename cannot be empty".to_string()));
        }
        Ok(())
    }
}

/// High-level function to verify a Debian package
pub async fn verify_debian_package(
    package: &str,
    version: &str,
    repo: &str,
    codename: &str,
    channel: &str,
    signed: bool,
) -> ManagerResult<()> {
    let config = DebianVerifyConfig {
        package: package.to_string(),
        version: version.to_string(),
        repo: repo.to_string(),
        codename: codename.to_string(),
        channel: channel.to_string(),
        signed,
    };

    let verifier = DebianVerifier::new(config);
    verifier.verify().await
}

/// High-level function to verify a Docker image
pub async fn verify_docker_image(
    package: &str,
    version: &str,
    repo: &str,
    codename: &str,
    suffix: &str,
) -> ManagerResult<()> {
    let config = DockerVerifyConfig {
        package: package.to_string(),
        version: version.to_string(),
        repo: repo.to_string(),
        codename: codename.to_string(),
        suffix: suffix.to_string(),
    };

    let verifier = DockerVerifier::new(config);
    verifier.verify().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debian_config_validation() {
        let config = DebianVerifyConfig {
            package: "mina-daemon".to_string(),
            version: "1.0.0".to_string(),
            repo: "packages.o1test.net".to_string(),
            codename: "bullseye".to_string(),
            channel: "stable".to_string(),
            signed: false,
        };

        let verifier = DebianVerifier::new(config);
        assert!(verifier.validate_config().is_ok());
    }

    #[test]
    fn test_docker_config_validation() {
        let config = DockerVerifyConfig {
            package: "mina-daemon".to_string(),
            version: "1.0.0".to_string(),
            repo: "gcr.io/o1labs-192920".to_string(),
            codename: "bullseye".to_string(),
            suffix: "-devnet".to_string(),
        };

        let verifier = DockerVerifier::new(config);
        assert!(verifier.validate_config().is_ok());
    }

    #[test]
    fn test_get_test_commands() {
        let config = DebianVerifyConfig {
            package: "mina-archive-devnet".to_string(),
            version: "1.0.0".to_string(),
            repo: "packages.o1test.net".to_string(),
            codename: "bullseye".to_string(),
            channel: "stable".to_string(),
            signed: false,
        };

        let verifier = DebianVerifier::new(config);
        let commands = verifier.get_test_commands();
        
        assert_eq!(commands, vec![
            "mina-archive --version".to_string(),
            "mina-archive --help".to_string(),
        ]);
    }

    #[test]
    fn test_get_applications() {
        let config = DockerVerifyConfig {
            package: "mina-rosetta".to_string(),
            version: "1.0.0".to_string(),
            repo: "gcr.io/o1labs-192920".to_string(),
            codename: "bullseye".to_string(),
            suffix: "-devnet".to_string(),
        };

        let verifier = DockerVerifier::new(config);
        let apps = verifier.get_applications();
        
        assert_eq!(apps, vec![
            "mina".to_string(),
            "mina-archive".to_string(),
            "mina-rosetta".to_string(),
        ]);
    }
}