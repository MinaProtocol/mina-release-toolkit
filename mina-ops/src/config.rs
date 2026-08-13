//! Project registry.
//!
//! A project is a configuration entry, never a code path. Adding a second
//! repository must not require touching any adapter.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{OpsError, OpsResult};

/// The example file doubles as the built-in default, so the two cannot drift.
const BUILTIN_REGISTRY: &str = include_str!("../projects.example.yaml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    pub projects: BTreeMap<String, Project>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub repo: String,
    pub buildkite: BuildkiteConfig,
    pub apt: AptConfig,
    pub docker: DockerConfig,
    pub defaults: Defaults,
    /// How to reach the CI cache. Absent by default: the real host and mount
    /// point belong in the operator's own configuration, not in this public
    /// repository.
    #[serde(default)]
    pub cache: Option<crate::adapters::hetzner::CacheConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildkiteConfig {
    pub org: String,
    #[serde(default)]
    pub pipelines: Vec<String>,
    /// Pipeline `mina-ops nightly` reports on when none is named.
    #[serde(default)]
    pub nightly_pipeline: Option<String>,
    /// Branch filter for that pipeline. `None` means every branch, which
    /// mixes release branches into the comparison.
    #[serde(default)]
    pub nightly_branch: Option<String>,
    /// Pipeline the hardfork package-generation form triggers.
    #[serde(default)]
    pub hardfork_pipeline: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AptConfig {
    pub region: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    pub gcr: String,
    pub docker_io: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub channel: String,
    pub artifacts: Vec<String>,
    pub codenames: Vec<String>,
    #[serde(default)]
    pub profile: Option<String>,
}

/// Where a registry was loaded from. Reported so a surprising result can be
/// traced back to the file that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrySource {
    Builtin,
    File(PathBuf),
}

impl std::fmt::Display for RegistrySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistrySource::Builtin => write!(f, "built-in defaults"),
            RegistrySource::File(p) => write!(f, "{}", p.display()),
        }
    }
}

impl Registry {
    /// Resolution order: explicit path, then `MINA_OPS_CONFIG`, then
    /// `~/.config/mina-ops/projects.yaml`, then the built-in defaults.
    pub fn load(explicit: Option<&Path>) -> OpsResult<(Self, RegistrySource)> {
        if let Some(path) = explicit {
            return Self::from_file(path);
        }
        if let Some(env_path) = std::env::var_os("MINA_OPS_CONFIG") {
            return Self::from_file(Path::new(&env_path));
        }
        if let Some(path) = user_config_path() {
            if path.exists() {
                return Self::from_file(&path);
            }
        }
        Ok((Self::builtin()?, RegistrySource::Builtin))
    }

    pub fn builtin() -> OpsResult<Self> {
        serde_yaml::from_str(BUILTIN_REGISTRY)
            .map_err(|e| OpsError::Config(format!("built-in registry is not valid: {e}")))
    }

    fn from_file(path: &Path) -> OpsResult<(Self, RegistrySource)> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| OpsError::Config(format!("cannot read {}: {e}", path.display())))?;
        let registry: Registry = serde_yaml::from_str(&text)
            .map_err(|e| OpsError::Config(format!("cannot parse {}: {e}", path.display())))?;
        Ok((registry, RegistrySource::File(path.to_path_buf())))
    }

    pub fn project(&self, name: &str) -> OpsResult<&Project> {
        self.projects.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.projects.keys().map(|s| s.as_str()).collect();
            OpsError::Config(format!(
                "unknown project '{name}'. Known projects: {}",
                known.join(", ")
            ))
        })
    }
}

pub fn user_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mina-ops").join("projects.yaml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_parses_and_has_mina() {
        let registry = Registry::builtin().expect("built-in registry must parse");
        let mina = registry.project("mina").expect("mina must be present");
        assert_eq!(mina.buildkite.org, "o-1-labs-2");
        assert_eq!(mina.apt.region, "us-west-2");
        assert!(!mina.defaults.codenames.is_empty());
    }

    #[test]
    fn unknown_project_lists_known_ones() {
        let registry = Registry::builtin().unwrap();
        let err = registry.project("nope").unwrap_err().to_string();
        assert!(
            err.contains("mina"),
            "error should list known projects: {err}"
        );
    }

    #[test]
    fn explicit_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.yaml");
        std::fs::write(
            &path,
            r#"
projects:
  other:
    repo: Example/other
    buildkite:
      org: example-org
    apt:
      region: eu-west-1
    docker:
      gcr: gcr.io/example
      docker_io: docker.io/example
    defaults:
      channel: unstable
      artifacts: [mina-daemon]
      codenames: [noble]
"#,
        )
        .unwrap();

        let (registry, source) = Registry::load(Some(&path)).unwrap();
        assert!(registry.project("other").is_ok());
        assert!(registry.project("mina").is_err());
        assert_eq!(source, RegistrySource::File(path));
    }
}
