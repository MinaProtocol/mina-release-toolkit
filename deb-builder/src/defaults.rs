use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub vendor: Option<String>,
    pub license: Option<String>,
    pub package_authors: Option<String>,
    pub package_maintainer: Option<String>,
    pub package_description: Option<String>,
    pub package_section: Option<String>,
    pub package_priority: Option<String>,
    pub package_homepage: Option<String>,
    pub package_installed_size: Option<String>,
    pub package_source: Option<String>,
    pub architecture: Option<String>,
    pub depends: Option<Vec<String>>,
    pub suggested_depends: Option<Vec<String>>,
    pub recommended_depends: Option<Vec<String>>,
    pub pre_depends: Option<Vec<String>>,
    pub conflicts: Option<Vec<String>>,
    pub replaces: Option<Vec<String>>,
    pub provides: Option<Vec<String>>,
    pub githash: Option<String>,
    pub buildurl: Option<String>,
}

impl Defaults {
    pub fn load(path: Option<&str>) -> Result<Self> {
        match path {
            None => Ok(Self::default()),
            Some(p) => {
                let path = Path::new(p);
                if !path.exists() {
                    anyhow::bail!("File ({}) does not exist or permission denied", p);
                }
                log::info!("Loading defaults from {} ...", p);
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to read defaults file ({})", p))?;
                serde_json::from_str(&text)
                    .with_context(|| format!("Failed to parse defaults file ({})", p))
            }
        }
    }
}
