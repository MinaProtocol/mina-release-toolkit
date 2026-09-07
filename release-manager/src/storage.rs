use crate::errors::{ManagerError, ManagerResult};
use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub enum StorageBackend {
    Local,
    Gs,
    Hetzner {
        user: String,
        host: String,
        key_path: String,
    },
}

impl StorageBackend {
    pub fn from_str(backend: &str) -> ManagerResult<Self> {
        match backend {
            "local" => Ok(StorageBackend::Local),
            "gs" => Ok(StorageBackend::Gs),
            "hetzner" => {
                let user = std::env::var("HETZNER_USER").unwrap_or_else(|_| "u434410".to_string());
                let host = std::env::var("HETZNER_HOST")
                    .unwrap_or_else(|_| "u434410-sub2.your-storagebox.de".to_string());
                let key_path = std::env::var("HETZNER_KEY").unwrap_or_else(|_| {
                    format!("{}/.ssh/id_rsa", std::env::var("HOME").unwrap_or_default())
                });

                Ok(StorageBackend::Hetzner {
                    user,
                    host,
                    key_path,
                })
            }
            _ => Err(ManagerError::UnsupportedBackend(backend.to_string())),
        }
    }

    pub fn root_path(&self) -> &str {
        match self {
            StorageBackend::Local => "/var/storagebox/",
            StorageBackend::Gs => "gs://buildkite_k8s/coda/shared",
            StorageBackend::Hetzner { .. } => {
                "/home/o1labs-generic/pvc-4d294645-6466-4260-b933-1b909ff9c3a1"
            }
        }
    }
}

#[async_trait]
pub trait StorageOperations {
    async fn list(&self, path: &str) -> ManagerResult<Vec<String>>;
    async fn md5(&self, path: &str) -> ManagerResult<String>;
    async fn download(&self, remote_path: &str, local_path: &str) -> ManagerResult<()>;
    async fn upload(&self, local_path: &str, remote_path: &str) -> ManagerResult<()>;
}

pub struct StorageClient {
    pub backend: StorageBackend,
}

impl StorageClient {
    pub fn new(backend: StorageBackend) -> Self {
        Self { backend }
    }

    async fn run_command(&self, cmd: &mut Command) -> ManagerResult<String> {
        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ManagerError::CommandFailed(stderr.to_string()));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

/// Extracts the MD5 digest from `gcloud storage hash --format=value(md5_hash)`
/// output, which prints one bare digest per line. A wildcard matching several
/// objects therefore yields several lines and the first one wins, preserving the
/// behaviour of the previous `gsutil hash` parser.
fn parse_md5_output(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

#[async_trait]
impl StorageOperations for StorageClient {
    async fn list(&self, path: &str) -> ManagerResult<Vec<String>> {
        let output = match &self.backend {
            StorageBackend::Local => {
                let mut cmd = Command::new("ls");
                cmd.arg(path);
                self.run_command(&mut cmd).await?
            }
            StorageBackend::Gs => {
                let mut cmd = Command::new("gcloud");
                cmd.args(["storage", "ls", path]);
                self.run_command(&mut cmd).await?
            }
            StorageBackend::Hetzner {
                user,
                host,
                key_path,
            } => {
                let mut cmd = Command::new("ssh");
                cmd.args([
                    "-p",
                    "23",
                    "-i",
                    key_path,
                    &format!("{}@{}", user, host),
                    &format!("ls {}", shell_escape::escape(path.into())),
                ]);
                self.run_command(&mut cmd).await?
            }
        };

        Ok(output.lines().map(|s| s.to_string()).collect())
    }

    async fn md5(&self, path: &str) -> ManagerResult<String> {
        let output = match &self.backend {
            StorageBackend::Local => {
                let mut cmd = Command::new("md5sum");
                cmd.arg(path);
                let result = self.run_command(&mut cmd).await?;
                result.split_whitespace().next().unwrap_or("").to_string()
            }
            StorageBackend::Gs => {
                let mut cmd = Command::new("gcloud");
                cmd.args([
                    "storage",
                    "hash",
                    "--hex",
                    "--skip-crc32c",
                    "--format=value(md5_hash)",
                    path,
                ]);
                let result = self.run_command(&mut cmd).await?;

                // Parse `gcloud storage hash` output
                if let Some(hash) = parse_md5_output(&result) {
                    return Ok(hash);
                }
                return Err(ManagerError::StorageError(
                    "Could not parse MD5 hash".to_string(),
                ));
            }
            StorageBackend::Hetzner {
                user,
                host,
                key_path,
            } => {
                let mut cmd = Command::new("ssh");
                cmd.args([
                    "-p",
                    "23",
                    "-i",
                    key_path,
                    &format!("{}@{}", user, host),
                    &format!("md5sum {}", shell_escape::escape(path.into())),
                ]);
                let result = self.run_command(&mut cmd).await?;
                result.split_whitespace().next().unwrap_or("").to_string()
            }
        };

        Ok(output)
    }

    async fn download(&self, remote_path: &str, local_path: &str) -> ManagerResult<()> {
        match &self.backend {
            StorageBackend::Local => {
                let mut cmd = Command::new("cp");
                cmd.args([remote_path, local_path]);
                self.run_command(&mut cmd).await?;
            }
            StorageBackend::Gs => {
                let mut cmd = Command::new("gcloud");
                cmd.args(["storage", "cp", remote_path, local_path]);
                self.run_command(&mut cmd).await?;
            }
            StorageBackend::Hetzner {
                user,
                host,
                key_path,
            } => {
                // First list files to get actual file names
                let list_cmd = format!("ls {}", shell_escape::escape(remote_path.into()));
                let mut ssh_cmd = Command::new("ssh");
                ssh_cmd.args([
                    "-p",
                    "23",
                    "-i",
                    key_path,
                    &format!("{}@{}", user, host),
                    &list_cmd,
                ]);
                let files = self.run_command(&mut ssh_cmd).await?;

                // Download each file using rsync
                for file in files.lines() {
                    let file = file.trim();
                    if !file.is_empty() {
                        let mut rsync_cmd = Command::new("rsync");
                        rsync_cmd.args([
                            "-avz",
                            "--rsh",
                            &format!("ssh -p 23 -i {}", key_path),
                            &format!("{}@{}:{}", user, host, file),
                            local_path,
                        ]);
                        self.run_command(&mut rsync_cmd).await?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn upload(&self, local_path: &str, remote_path: &str) -> ManagerResult<()> {
        match &self.backend {
            StorageBackend::Local => {
                let mut cmd = Command::new("cp");
                cmd.args([local_path, remote_path]);
                self.run_command(&mut cmd).await?;
            }
            StorageBackend::Gs => {
                let mut cmd = Command::new("gcloud");
                cmd.args(["storage", "cp", local_path, remote_path]);
                self.run_command(&mut cmd).await?;
            }
            StorageBackend::Hetzner {
                user,
                host,
                key_path,
            } => {
                let mut cmd = Command::new("rsync");
                cmd.args([
                    "-avz",
                    "-e",
                    &format!("ssh -p 23 -i {}", key_path),
                    local_path,
                    &format!("{}@{}:{}", user, host, remote_path),
                ]);
                self.run_command(&mut cmd).await?;
            }
        }

        Ok(())
    }
}

pub async fn get_cached_debian_or_download(
    storage: &StorageClient,
    artifact: &str,
    codename: &str,
    network: Option<&str>,
    buildkite_build_id: &str,
    cache_folder: &Path,
) -> ManagerResult<()> {
    use crate::artifacts::get_artifact_with_suffix;

    let artifact_full_name = get_artifact_with_suffix(artifact, network, None);
    let remote_path = format!(
        "{}/{}/debians/{}/{}_*",
        storage.backend.root_path(),
        buildkite_build_id,
        codename,
        artifact_full_name
    );

    // Check if files exist
    let files = storage.list(&remote_path).await?;
    if files.is_empty() {
        return Err(ManagerError::ArtifactNotFound(format!(
            "No debian package found for {} (build: {})",
            artifact_full_name, buildkite_build_id
        )));
    }

    // Get target hash
    let target_hash = storage.md5(&remote_path).await?;

    // Create cache directory
    let cache_dir = cache_folder.join(codename);
    tokio::fs::create_dir_all(&cache_dir).await?;

    println!(
        " 🗂️  Checking cache for {}/{} Debian package",
        codename, artifact_full_name
    );

    // Check if already cached with correct hash
    let cached_files = tokio::fs::read_dir(&cache_dir).await;
    if let Ok(mut entries) = cached_files {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();
            if file_name_str.starts_with(&format!("{}_", artifact_full_name)) {
                // Check if hash matches
                let file_path = entry.path();
                let mut cmd = Command::new("md5sum");
                cmd.arg(&file_path);

                if let Ok(output) = cmd.output().await {
                    let hash = String::from_utf8_lossy(&output.stdout);
                    if let Some(local_hash) = hash.split_whitespace().next() {
                        if local_hash == target_hash {
                            println!(
                                "   🗂️  {} Debian package already cached. Skipping download.",
                                artifact_full_name
                            );
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    println!(
        "   📂  {} Debian package is not cached. Downloading from {:?}.",
        artifact_full_name, storage.backend
    );
    storage
        .download(&remote_path, cache_dir.to_str().unwrap())
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_md5_output_single_line() {
        assert_eq!(
            parse_md5_output("d41d8cd98f00b204e9800998ecf8427e\n"),
            Some("d41d8cd98f00b204e9800998ecf8427e".to_string())
        );
    }

    #[test]
    fn test_parse_md5_output_returns_first_of_many() {
        let output = "d41d8cd98f00b204e9800998ecf8427e\n9e107d9d372bb6826bd81d3542a419d6\n";
        assert_eq!(
            parse_md5_output(output),
            Some("d41d8cd98f00b204e9800998ecf8427e".to_string())
        );
    }

    #[test]
    fn test_parse_md5_output_empty() {
        assert_eq!(parse_md5_output(""), None);
    }

    #[test]
    fn test_parse_md5_output_whitespace_only() {
        assert_eq!(parse_md5_output("  \n\t\n \n"), None);
    }
}
