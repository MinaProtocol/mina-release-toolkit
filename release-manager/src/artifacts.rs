use regex::Regex;
use crate::errors::{ManagerError, ManagerResult};

#[derive(Debug, Clone, PartialEq)]
pub enum Artifact {
    MinaDaemon,
    MinaArchive,
    MinaRosetta,
    MinaLogproc,
}

impl Artifact {
    pub fn from_str(s: &str) -> ManagerResult<Self> {
        match s {
            "mina-daemon" => Ok(Artifact::MinaDaemon),
            "mina-archive" => Ok(Artifact::MinaArchive),
            "mina-rosetta" => Ok(Artifact::MinaRosetta),
            "mina-logproc" => Ok(Artifact::MinaLogproc),
            _ => Err(ManagerError::UnknownArtifact(s.to_string())),
        }
    }
    
    pub fn as_str(&self) -> &'static str {
        match self {
            Artifact::MinaDaemon => "mina-daemon",
            Artifact::MinaArchive => "mina-archive",
            Artifact::MinaRosetta => "mina-rosetta",
            Artifact::MinaLogproc => "mina-logproc",
        }
    }
}

pub fn get_suffix(artifact: &str, network: Option<&str>) -> String {
    match artifact {
        "mina-daemon" | "mina-rosetta" | "mina-archive" => {
            if let Some(network) = network {
                format!("-{}", network)
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

pub fn get_artifact_with_suffix(artifact: &str, network: Option<&str>) -> String {
    match artifact {
        "mina-daemon" => {
            if let Some(network) = network {
                format!("mina-{}", network)
            } else {
                artifact.to_string()
            }
        }
        "mina-rosetta" => {
            if let Some(network) = network {
                format!("mina-rosetta-{}", network)
            } else {
                artifact.to_string()
            }
        }
        "mina-archive" => {
            if let Some(network) = network {
                format!("mina-archive-{}", network)
            } else {
                artifact.to_string()
            }
        }
        _ => artifact.to_string(),
    }
}

pub fn calculate_debian_version(artifact: &str, target_version: &str, codename: &str, network: Option<&str>) -> String {
    let network_suffix = get_suffix(artifact, network);
    format!("{}:{}-{}{}", artifact, target_version, codename, network_suffix)
}

pub fn extract_version_from_deb(deb_file: &str) -> ManagerResult<String> {
    let re = Regex::new(r".*_([^_]*)\.deb$").map_err(|e| ManagerError::ValidationError(e.to_string()))?;
    
    if let Some(captures) = re.captures(deb_file) {
        if let Some(version) = captures.get(1) {
            return Ok(version.as_str().to_string());
        }
    }
    
    Err(ManagerError::ValidationError(format!("Could not extract version from: {}", deb_file)))
}

pub fn calculate_docker_tag(
    publish_to_docker_io: bool,
    artifact: &str,
    target_version: &str,
    codename: &str,
    network: Option<&str>,
) -> String {
    let network_suffix = get_suffix(artifact, network);
    let repo = if publish_to_docker_io {
        "docker.io/minaprotocol"
    } else {
        "gcr.io/o1labs-192920"
    };
    
    format!("{}/{}:{}-{}{}", repo, artifact, target_version, codename, network_suffix)
}

pub fn get_repo(publish_to_docker_io: bool) -> &'static str {
    if publish_to_docker_io {
        "docker.io/minaprotocol"
    } else {
        "gcr.io/o1labs-192920"
    }
}

pub fn combine_docker_suffixes(network: &str, docker_suffix: Option<&str>) -> String {
    if let Some(suffix) = docker_suffix {
        format!("-{}-{}", network, suffix)
    } else {
        format!("-{}", network)
    }
}

pub fn parse_artifact_list(artifacts: &str) -> ManagerResult<Vec<Artifact>> {
    artifacts
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(Artifact::from_str)
        .collect()
}

pub fn parse_string_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_suffix() {
        assert_eq!(get_suffix("mina-daemon", Some("devnet")), "-devnet");
        assert_eq!(get_suffix("mina-logproc", Some("devnet")), "");
        assert_eq!(get_suffix("mina-daemon", None), "");
    }

    #[test]
    fn test_get_artifact_with_suffix() {
        assert_eq!(get_artifact_with_suffix("mina-daemon", Some("devnet")), "mina-devnet");
        assert_eq!(get_artifact_with_suffix("mina-archive", Some("mainnet")), "mina-archive-mainnet");
        assert_eq!(get_artifact_with_suffix("mina-logproc", Some("devnet")), "mina-logproc");
    }

    #[test]
    fn test_extract_version_from_deb() {
        assert_eq!(
            extract_version_from_deb("mina-daemon_1.0.0-bullseye.deb").unwrap(),
            "1.0.0-bullseye"
        );
    }

    #[test]
    fn test_parse_artifact_list() {
        let artifacts = parse_artifact_list("mina-daemon,mina-archive").unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0], Artifact::MinaDaemon);
        assert_eq!(artifacts[1], Artifact::MinaArchive);
    }
}