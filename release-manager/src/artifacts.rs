use crate::errors::{ManagerError, ManagerResult};
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Artifact {
    MinaDaemon,
    MinaArchive,
    MinaRosetta,
    MinaLogproc,
    MinaConfig,
    MinaAutomode,
    MinaPrefork,
    MinaPostfork,
    MinaGeneric,
    RosettaGeneric,
    MinaPostforkMesa,
    MinaPreforkMesa,
    Minimina,
}

impl Artifact {
    pub fn from_str(s: &str) -> ManagerResult<Self> {
        match s {
            "mina-daemon" => Ok(Artifact::MinaDaemon),
            "mina-archive" => Ok(Artifact::MinaArchive),
            "mina-rosetta" => Ok(Artifact::MinaRosetta),
            "mina-logproc" => Ok(Artifact::MinaLogproc),
            "mina-config" => Ok(Artifact::MinaConfig),
            "mina-automode" => Ok(Artifact::MinaAutomode),
            "mina-prefork" => Ok(Artifact::MinaPrefork),
            "mina-postfork" => Ok(Artifact::MinaPostfork),
            "mina-generic" => Ok(Artifact::MinaGeneric),
            "rosetta-generic" => Ok(Artifact::RosettaGeneric),
            "mina-postfork-mesa" => Ok(Artifact::MinaPostforkMesa),
            "mina-prefork-mesa" => Ok(Artifact::MinaPreforkMesa),
            "minimina" => Ok(Artifact::Minimina),
            _ => Err(ManagerError::UnknownArtifact(s.to_string())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Artifact::MinaDaemon => "mina-daemon",
            Artifact::MinaArchive => "mina-archive",
            Artifact::MinaRosetta => "mina-rosetta",
            Artifact::MinaLogproc => "mina-logproc",
            Artifact::MinaConfig => "mina-config",
            Artifact::MinaAutomode => "mina-automode",
            Artifact::MinaPrefork => "mina-prefork",
            Artifact::MinaPostfork => "mina-postfork",
            Artifact::MinaGeneric => "mina-generic",
            Artifact::RosettaGeneric => "rosetta-generic",
            Artifact::MinaPostforkMesa => "mina-postfork-mesa",
            Artifact::MinaPreforkMesa => "mina-prefork-mesa",
            Artifact::Minimina => "minimina",
        }
    }

}

/// Build profiles understood by manager.sh (`lightnet`, `instrumented`).
/// `None` means the default profile.
fn profile_part(profile: Option<&str>) -> &str {
    match profile {
        Some("lightnet") => "-lightnet",
        Some("instrumented") => "-instrumented",
        _ => "",
    }
}

/// Trailing suffix appended to the package's base version, e.g. `-devnet` or
/// `-devnet-lightnet`. Matches manager.sh's `get_suffix()`.
///
/// Returns the empty string when neither a network nor a profile applies to
/// the artifact — manager.sh's bash equivalent emits a stray `-` in that
/// case (`"-$network$profile"` with both empty), which we suppress here.
pub fn get_suffix(artifact: &str, network: Option<&str>, profile: Option<&str>) -> String {
    let prof = profile_part(profile);
    let net = network.map(|n| format!("-{}", n)).unwrap_or_default();
    match artifact {
        // Network + optional profile
        "mina-daemon" | "mina-generic" => format!("{}{}", net, prof),
        // Network only (profile ignored)
        "mina-rosetta"
        | "mina-archive"
        | "mina-config"
        | "mina-automode"
        | "mina-prefork"
        | "mina-postfork"
        | "rosetta-generic"
        | "mina-postfork-mesa"
        | "mina-prefork-mesa" => net,
        // Neither network nor profile
        _ => String::new(),
    }
}

/// Resolve an artifact name + network + profile to the actual package name
/// that ends up in the Debian repo / Docker registry. Matches manager.sh's
/// `get_artifact_with_suffix()`.
pub fn get_artifact_with_suffix(
    artifact: &str,
    network: Option<&str>,
    profile: Option<&str>,
) -> String {
    let net = network.unwrap_or("");
    match artifact {
        "mina-daemon" => match profile {
            Some("lightnet") | Some("instrumented") => {
                format!("mina-{}-{}", net, profile.unwrap())
            }
            _ => format!("mina-{}", net),
        },
        "mina-rosetta" => format!("mina-rosetta-{}", net),
        "mina-archive" => format!("mina-archive-{}", net),
        "mina-config" => format!("mina-{}-config", net),
        "mina-automode" => format!("mina-{}-automode", net),
        "mina-prefork" => format!("mina-{}-prefork-mesa", net),
        "mina-postfork" => format!("mina-{}-postfork-mesa", net),
        "mina-generic" => match profile {
            Some("lightnet") => format!("mina-{}-generic-lightnet", net),
            _ => format!("mina-{}-generic", net),
        },
        "rosetta-generic" => format!("mina-rosetta-{}-generic", net),
        "mina-postfork-mesa" => format!("mina-{}-postfork-mesa", net),
        "mina-prefork-mesa" => format!("mina-{}-prefork-mesa", net),
        _ => artifact.to_string(),
    }
}

/// CI builds some artifacts under a different Docker image name than the
/// artifact identifier. Mirrors manager.sh's `get_docker_image_name()`.
pub fn get_docker_image_name(artifact: &str) -> &str {
    match artifact {
        "mina-generic" => "mina-daemon",
        "rosetta-generic" => "mina-rosetta",
        other => other,
    }
}

pub fn calculate_debian_version(
    artifact: &str,
    target_version: &str,
    codename: &str,
    network: Option<&str>,
    arch: Option<&str>,
) -> String {
    let network_suffix = get_suffix(artifact, network, None);
    let arch_part = match arch {
        Some(a) if !a.is_empty() => format!("-{}", a),
        _ => String::new(),
    };
    format!(
        "{}:{}-{}{}{}",
        artifact, target_version, codename, network_suffix, arch_part
    )
}

pub fn extract_version_from_deb(deb_file: &str) -> ManagerResult<String> {
    let re = Regex::new(r".*_([^_]*)\.deb$")
        .map_err(|e| ManagerError::ValidationError(e.to_string()))?;

    if let Some(captures) = re.captures(deb_file) {
        if let Some(version) = captures.get(1) {
            return Ok(version.as_str().to_string());
        }
    }

    Err(ManagerError::ValidationError(format!(
        "Could not extract version from: {}",
        deb_file
    )))
}

pub fn get_arch_suffix(arch: &str) -> String {
    if arch.is_empty() || arch == "amd64" {
        String::new()
    } else {
        format!("-{}", arch)
    }
}

pub fn calculate_docker_tag(
    publish_to_docker_io: bool,
    artifact: &str,
    target_version: &str,
    codename: &str,
    network: Option<&str>,
    profile: Option<&str>,
    arch: Option<&str>,
) -> String {
    let docker_name = get_docker_image_name(artifact);
    let network_suffix = get_suffix(artifact, network, profile);
    let arch_suffix = arch.map(get_arch_suffix).unwrap_or_default();
    let repo = get_repo(publish_to_docker_io);
    format!(
        "{}/{}:{}-{}{}{}",
        repo, docker_name, target_version, codename, network_suffix, arch_suffix
    )
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
        assert_eq!(get_suffix("mina-daemon", Some("devnet"), None), "-devnet");
        assert_eq!(get_suffix("mina-logproc", Some("devnet"), None), "");
        assert_eq!(get_suffix("mina-daemon", None, None), "");
        assert_eq!(
            get_suffix("mina-daemon", Some("devnet"), Some("lightnet")),
            "-devnet-lightnet"
        );
        assert_eq!(
            get_suffix("mina-archive", Some("mainnet"), None),
            "-mainnet"
        );
        assert_eq!(get_suffix("minimina", Some("devnet"), None), "");
    }

    #[test]
    fn test_get_artifact_with_suffix() {
        assert_eq!(
            get_artifact_with_suffix("mina-daemon", Some("devnet"), None),
            "mina-devnet"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-daemon", Some("devnet"), Some("lightnet")),
            "mina-devnet-lightnet"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-archive", Some("mainnet"), None),
            "mina-archive-mainnet"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-config", Some("devnet"), None),
            "mina-devnet-config"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-automode", Some("mainnet"), None),
            "mina-mainnet-automode"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-prefork", Some("devnet"), None),
            "mina-devnet-prefork-mesa"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-postfork", Some("mainnet"), None),
            "mina-mainnet-postfork-mesa"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-generic", Some("devnet"), None),
            "mina-devnet-generic"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-generic", Some("devnet"), Some("lightnet")),
            "mina-devnet-generic-lightnet"
        );
        assert_eq!(
            get_artifact_with_suffix("rosetta-generic", Some("devnet"), None),
            "mina-rosetta-devnet-generic"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-postfork-mesa", Some("mainnet"), None),
            "mina-mainnet-postfork-mesa"
        );
        assert_eq!(
            get_artifact_with_suffix("mina-prefork-mesa", Some("mainnet"), None),
            "mina-mainnet-prefork-mesa"
        );
        assert_eq!(get_artifact_with_suffix("minimina", None, None), "minimina");
    }

    #[test]
    fn test_get_docker_image_name() {
        assert_eq!(get_docker_image_name("mina-generic"), "mina-daemon");
        assert_eq!(get_docker_image_name("rosetta-generic"), "mina-rosetta");
        assert_eq!(get_docker_image_name("mina-daemon"), "mina-daemon");
        assert_eq!(get_docker_image_name("mina-archive"), "mina-archive");
        assert_eq!(get_docker_image_name("minimina"), "minimina");
    }

    #[test]
    fn test_calculate_debian_version_with_arch() {
        assert_eq!(
            calculate_debian_version(
                "mina-daemon",
                "1.0.0",
                "bullseye",
                Some("devnet"),
                Some("amd64")
            ),
            "mina-daemon:1.0.0-bullseye-devnet-amd64"
        );
        assert_eq!(
            calculate_debian_version("mina-logproc", "1.0.0", "bullseye", None, None),
            "mina-logproc:1.0.0-bullseye"
        );
    }

    #[test]
    fn test_calculate_docker_tag_with_docker_name_mapping() {
        // mina-generic's docker image is published as mina-daemon
        assert_eq!(
            calculate_docker_tag(
                false,
                "mina-generic",
                "1.0.0",
                "bullseye",
                Some("devnet"),
                None,
                None
            ),
            "gcr.io/o1labs-192920/mina-daemon:1.0.0-bullseye-devnet"
        );
        assert_eq!(
            calculate_docker_tag(
                false,
                "rosetta-generic",
                "1.0.0",
                "bullseye",
                Some("devnet"),
                None,
                None
            ),
            "gcr.io/o1labs-192920/mina-rosetta:1.0.0-bullseye-devnet"
        );
        // Regular artifact: no name mapping
        assert_eq!(
            calculate_docker_tag(
                true,
                "mina-archive",
                "1.0.0",
                "focal",
                Some("mainnet"),
                None,
                None
            ),
            "docker.io/minaprotocol/mina-archive:1.0.0-focal-mainnet"
        );
        // Profile passthrough
        assert_eq!(
            calculate_docker_tag(
                false,
                "mina-daemon",
                "1.0.0",
                "bullseye",
                Some("devnet"),
                Some("lightnet"),
                None
            ),
            "gcr.io/o1labs-192920/mina-daemon:1.0.0-bullseye-devnet-lightnet"
        );
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
        let artifacts =
            parse_artifact_list("mina-daemon,mina-archive,mina-generic,minimina").unwrap();
        assert_eq!(artifacts.len(), 4);
        assert_eq!(artifacts[0], Artifact::MinaDaemon);
        assert_eq!(artifacts[1], Artifact::MinaArchive);
        assert_eq!(artifacts[2], Artifact::MinaGeneric);
        assert_eq!(artifacts[3], Artifact::Minimina);
    }

}
