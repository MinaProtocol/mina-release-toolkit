use crate::errors::{ManagerResult, ManagerError};
use tokio::process::Command as AsyncCommand;

/// Configuration for Docker image promotion
#[derive(Debug, Clone)]
pub struct DockerPromoteConfig {
    /// Docker image name
    pub name: String,
    /// Source version/tag
    pub source_version: String,
    /// Target version/tag
    pub target_version: String,
    /// Whether to publish to docker.io (vs gcr.io)
    pub publish_to_docker_io: bool,
    /// Quiet mode (minimal output)
    pub quiet: bool,
}

/// Docker image promoter
pub struct DockerPromoter {
    config: DockerPromoteConfig,
}

const GCR_REGISTRY: &str = "gcr.io/o1labs-192920";
const DOCKER_IO_REGISTRY: &str = "docker.io/minaprotocol";

impl DockerPromoter {
    /// Create a new DockerPromoter
    pub fn new(config: DockerPromoteConfig) -> Self {
        Self { config }
    }

    /// Promote Docker image from source to target version/registry
    pub async fn promote(&self) -> ManagerResult<()> {
        self.validate_config()?;

        if !self.config.quiet {
            println!(" 🐋 Promoting Docker image:");
            println!("    📦 Name: {}", self.config.name);
            println!("    🏷️  Source: {}", self.config.source_version);
            println!("    🎯 Target: {}", self.config.target_version);
            println!("    🌐 Publish to docker.io: {}", self.config.publish_to_docker_io);
        }

        let config = if self.config.publish_to_docker_io {
            DockerRegistryConfig {
                source_registry: GCR_REGISTRY.to_string(),
                target_registry: DOCKER_IO_REGISTRY.to_string(),
                image_name: self.config.name.clone(),
                source_tag: self.config.source_version.clone(),
                target_tag: self.config.target_version.clone(),
            }
        } else {
            DockerRegistryConfig {
                source_registry: GCR_REGISTRY.to_string(),
                target_registry: GCR_REGISTRY.to_string(),
                image_name: self.config.name.clone(),
                source_tag: self.config.source_version.clone(),
                target_tag: self.config.target_version.clone(),
            }
        };

        let manager = DockerRegistryManager::new(config);
        manager.cross_registry_promote().await?;

        if !self.config.quiet {
            println!("    ✅ Docker image promotion successful");
        }

        Ok(())
    }

    /// Validate configuration parameters
    fn validate_config(&self) -> ManagerResult<()> {
        if self.config.name.is_empty() {
            return Err(ManagerError::ValidationError("Name cannot be empty".to_string()));
        }
        if self.config.source_version.is_empty() {
            return Err(ManagerError::ValidationError("Source version cannot be empty".to_string()));
        }
        if self.config.target_version.is_empty() {
            return Err(ManagerError::ValidationError("Target version cannot be empty".to_string()));
        }
        Ok(())
    }
}

/// High-level function to promote a Docker image
pub async fn promote_docker_image(
    name: &str,
    source_version: &str,
    target_version: &str,
    publish_to_docker_io: bool,
    quiet: bool,
) -> ManagerResult<()> {
    let config = DockerPromoteConfig {
        name: name.to_string(),
        source_version: source_version.to_string(),
        target_version: target_version.to_string(),
        publish_to_docker_io,
        quiet,
    };

    let promoter = DockerPromoter::new(config);
    promoter.promote().await
}

/// Configuration for Docker registry management
#[derive(Debug, Clone)]
pub struct DockerRegistryConfig {
    /// Source registry
    pub source_registry: String,
    /// Target registry
    pub target_registry: String,
    /// Image name
    pub image_name: String,
    /// Source tag
    pub source_tag: String,
    /// Target tag
    pub target_tag: String,
}

/// Advanced Docker registry manager for cross-registry promotion
pub struct DockerRegistryManager {
    config: DockerRegistryConfig,
}

impl DockerRegistryManager {
    /// Create a new DockerRegistryManager
    pub fn new(config: DockerRegistryConfig) -> Self {
        Self { config }
    }

    /// Promote image between different registries
    pub async fn cross_registry_promote(&self) -> ManagerResult<()> {
        self.validate_config()?;

        let source_image = format!("{}/{}:{}", 
                                  self.config.source_registry, 
                                  self.config.image_name, 
                                  self.config.source_tag);
        
        let target_image = format!("{}/{}:{}", 
                                  self.config.target_registry, 
                                  self.config.image_name, 
                                  self.config.target_tag);

        println!(" 🔄 Cross-registry promotion:");
        println!("    📥 Source: {}", source_image);
        println!("    📤 Target: {}", target_image);

        // Pull from source registry
        self.pull_image(&source_image).await?;

        // Tag for target registry
        self.tag_image(&source_image, &target_image).await?;

        // Push to target registry
        self.push_image(&target_image).await?;

        println!("    ✅ Cross-registry promotion successful");
        Ok(())
    }

    /// Pull Docker image
    async fn pull_image(&self, image: &str) -> ManagerResult<()> {
        println!("    📥 Pulling: {}", image);

        let mut cmd = AsyncCommand::new("docker");
        cmd.arg("pull").arg(image);

        let output = cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to pull image: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ManagerError::CommandFailed(format!(
                "Docker pull failed: {}", stderr
            )));
        }

        Ok(())
    }

    /// Tag Docker image
    async fn tag_image(&self, source: &str, target: &str) -> ManagerResult<()> {
        println!("    🏷️  Tagging: {} -> {}", source, target);

        let mut cmd = AsyncCommand::new("docker");
        cmd.arg("tag").arg(source).arg(target);

        let output = cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to tag image: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ManagerError::CommandFailed(format!(
                "Docker tag failed: {}", stderr
            )));
        }

        Ok(())
    }

    /// Push Docker image
    async fn push_image(&self, image: &str) -> ManagerResult<()> {
        println!("    📤 Pushing: {}", image);

        let mut cmd = AsyncCommand::new("docker");
        cmd.arg("push").arg(image);

        let output = cmd.output().await
            .map_err(|e| ManagerError::CommandFailed(format!("Failed to push image: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ManagerError::CommandFailed(format!(
                "Docker push failed: {}", stderr
            )));
        }

        Ok(())
    }

    /// Validate configuration
    fn validate_config(&self) -> ManagerResult<()> {
        if self.config.source_registry.is_empty() {
            return Err(ManagerError::ValidationError("Source registry cannot be empty".to_string()));
        }
        if self.config.target_registry.is_empty() {
            return Err(ManagerError::ValidationError("Target registry cannot be empty".to_string()));
        }
        if self.config.image_name.is_empty() {
            return Err(ManagerError::ValidationError("Image name cannot be empty".to_string()));
        }
        if self.config.source_tag.is_empty() {
            return Err(ManagerError::ValidationError("Source tag cannot be empty".to_string()));
        }
        if self.config.target_tag.is_empty() {
            return Err(ManagerError::ValidationError("Target tag cannot be empty".to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        let config = DockerPromoteConfig {
            name: "mina-daemon".to_string(),
            source_version: "1.0.0-abc123".to_string(),
            target_version: "1.0.0".to_string(),
            publish_to_docker_io: false,
            quiet: false,
        };

        let promoter = DockerPromoter::new(config);
        assert!(promoter.validate_config().is_ok());
    }

    #[test]
    fn test_empty_name_validation() {
        let config = DockerPromoteConfig {
            name: "".to_string(),
            source_version: "1.0.0-abc123".to_string(),
            target_version: "1.0.0".to_string(),
            publish_to_docker_io: false,
            quiet: false,
        };

        let promoter = DockerPromoter::new(config);
        assert!(promoter.validate_config().is_err());
    }

    #[test]
    fn test_registry_config_validation() {
        let config = DockerRegistryConfig {
            source_registry: "gcr.io/o1labs-192920".to_string(),
            target_registry: "docker.io/minaprotocol".to_string(),
            image_name: "mina-daemon".to_string(),
            source_tag: "1.0.0-dev".to_string(),
            target_tag: "1.0.0".to_string(),
        };

        let manager = DockerRegistryManager::new(config);
        assert!(manager.validate_config().is_ok());
    }
}