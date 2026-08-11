//! The entity model. Everything is keyed by commit, because the commit is the
//! only identifier that all of Buildkite, the Debian repositories and the
//! Docker registries share.

use serde::{Deserialize, Serialize};

/// Result of one existence check.
///
/// `Unknown` exists so a check that could not run is never reported as
/// `Missing`. A missing Docker credential must not read as a missing image —
/// that mistake causes rebuilds of artifacts that were there all along.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub enum Presence {
    Present,
    Missing,
    Unknown(String),
}

impl Presence {
    pub fn is_present(&self) -> bool {
        matches!(self, Presence::Present)
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Presence::Unknown(_))
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            Presence::Present => "OK",
            Presence::Missing => "MISSING",
            Presence::Unknown(_) => "UNKNOWN",
        }
    }
}

/// How the version being reported on was arrived at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionSource {
    /// Supplied on the command line.
    Given,
    /// Parsed from the `.deb` artifacts of a Buildkite build.
    BuildkiteArtifacts,
    /// Found in a Debian repository by matching the commit's short hash.
    DebianRepository,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSummary {
    pub pipeline: String,
    pub number: u64,
    pub state: String,
    pub branch: String,
    pub commit: String,
    pub message: Option<String>,
    pub created_at: Option<String>,
    pub web_url: String,
    /// `None` when the artifact list was not requested.
    ///
    /// A count of zero is normal for Mina: its pipelines upload logs and
    /// tools to Buildkite and put the packages in the CI cache instead. Zero
    /// therefore means "no Buildkite artifacts", never "the packages were
    /// deleted".
    pub artifact_count: Option<usize>,
    /// Versions parsed from this build's `.deb` artifacts. Usually empty, for
    /// the same reason.
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebianEntry {
    pub package: String,
    pub version: String,
    pub codename: String,
    pub arch: String,
    pub component: String,
    pub bucket: String,
    pub presence: Presence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerEntry {
    pub image: String,
    pub tag: String,
    pub repo: String,
    pub presence: Presence,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Totals {
    pub present: usize,
    pub missing: usize,
    pub unknown: usize,
}

impl Totals {
    pub fn add(&mut self, presence: &Presence) {
        match presence {
            Presence::Present => self.present += 1,
            Presence::Missing => self.missing += 1,
            Presence::Unknown(_) => self.unknown += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.present + self.missing + self.unknown
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub project: String,
    pub commit: Option<String>,
    pub version: Option<String>,
    pub version_source: Option<VersionSource>,
    pub channel: String,
    pub network: String,
    pub profile: Option<String>,
    /// False when only builds were asked for. Consumers use it to tell "no
    /// packages are published" apart from "packages were not looked at".
    pub artifact_coverage_requested: bool,
    pub builds: Vec<BuildSummary>,
    pub debians: Vec<DebianEntry>,
    pub dockers: Vec<DockerEntry>,
    /// Anything that limited the answer: a skipped check, a missing tool, a
    /// truncated query. Never silently drop coverage.
    pub warnings: Vec<String>,
}

impl Inventory {
    pub fn debian_totals(&self) -> Totals {
        let mut t = Totals::default();
        for entry in &self.debians {
            t.add(&entry.presence);
        }
        t
    }

    pub fn docker_totals(&self) -> Totals {
        let mut t = Totals::default();
        for entry in &self.dockers {
            t.add(&entry.presence);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_is_not_counted_as_missing() {
        let mut t = Totals::default();
        t.add(&Presence::Present);
        t.add(&Presence::Missing);
        t.add(&Presence::Unknown("docker not on PATH".into()));
        assert_eq!(t.present, 1);
        assert_eq!(t.missing, 1);
        assert_eq!(t.unknown, 1);
        assert_eq!(t.total(), 3);
    }

    #[test]
    fn presence_serialises_with_its_reason() {
        let json = serde_json::to_string(&Presence::Unknown("no docker".into())).unwrap();
        assert_eq!(json, r#"{"status":"unknown","reason":"no docker"}"#);
        let plain = serde_json::to_string(&Presence::Present).unwrap();
        assert_eq!(plain, r#"{"status":"present"}"#);
    }

    #[test]
    fn a_build_distinguishes_unlisted_artifacts_from_none() {
        let build = |count: Option<usize>| BuildSummary {
            pipeline: "mina".into(),
            number: 1,
            state: "passed".into(),
            branch: "compatible".into(),
            commit: "abc".into(),
            message: None,
            created_at: None,
            web_url: String::new(),
            artifact_count: count,
            versions: vec![],
        };
        assert_eq!(build(None).artifact_count, None);
        assert_eq!(build(Some(0)).artifact_count, Some(0));
    }
}
