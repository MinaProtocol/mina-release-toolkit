//! Text rendering.
//!
//! The JSON form is the contract for scripts and agents; this is for people.
//! Both are produced from the same `Inventory`, so they cannot disagree.

use colored::*;

use crate::model::{Inventory, Presence, Totals, VersionSource};

pub fn render(inventory: &Inventory) -> String {
    let mut out = String::new();

    out.push_str(&format!("\nProject: {}\n", inventory.project));
    if let Some(commit) = &inventory.commit {
        out.push_str(&format!("Commit:  {commit}\n"));
    }
    match (&inventory.version, &inventory.version_source) {
        (Some(version), Some(source)) => {
            out.push_str(&format!("Version: {} ({})\n", version, describe(source)));
        }
        // Only an artifact query owes the reader a version.
        _ if inventory.artifact_coverage_requested => {
            out.push_str(&format!("Version: {}\n", "unresolved".yellow()));
        }
        _ => {}
    }
    if inventory.artifact_coverage_requested {
        out.push_str(&format!(
            "Channel: {} · network {}\n",
            inventory.channel, inventory.network
        ));
    }

    if !inventory.builds.is_empty() {
        out.push_str("\nBuildkite\n");
        for build in &inventory.builds {
            let state = colour_state(&build.state);
            let artifacts = match build.artifact_count {
                Some(n) => format!("{n} artifacts"),
                None => "artifacts not listed".dimmed().to_string(),
            };
            out.push_str(&format!(
                "  {:<24} #{:<7} {:<10} {:<28} {}\n",
                truncate(&build.pipeline, 24),
                build.number,
                state,
                truncate(&build.branch, 28),
                artifacts
            ));
            out.push_str(&format!("      {}\n", build.web_url.dimmed()));
        }
    }

    if !inventory.debians.is_empty() {
        out.push_str("\nDebian packages\n");
        let mut current_group = String::new();
        for entry in &inventory.debians {
            let group = format!("{} · {}", entry.bucket, entry.codename);
            if group != current_group {
                out.push_str(&format!("  {}\n", group.bold()));
                current_group = group;
            }
            out.push_str(&format!(
                "    {} {:<32} {:<6}\n",
                mark(&entry.presence),
                entry.package,
                entry.arch
            ));
            if let Presence::Unknown(reason) = &entry.presence {
                out.push_str(&format!("        {}\n", reason.dimmed()));
            }
        }
    }

    if !inventory.dockers.is_empty() {
        out.push_str("\nDocker images\n");
        for entry in &inventory.dockers {
            out.push_str(&format!(
                "    {} {}/{}:{}\n",
                mark(&entry.presence),
                entry.repo,
                entry.image,
                entry.tag
            ));
        }
    }

    out.push_str("\nSummary\n");
    out.push_str(&format!(
        "  Debian: {}\n",
        summarise_totals(&inventory.debian_totals())
    ));
    out.push_str(&format!(
        "  Docker: {}\n",
        summarise_totals(&inventory.docker_totals())
    ));

    if !inventory.warnings.is_empty() {
        out.push_str("\nLimits and warnings\n");
        for warning in &inventory.warnings {
            out.push_str(&format!("  {} {}\n", "!".yellow(), warning));
        }
    }

    out.push('\n');
    out
}

fn describe(source: &VersionSource) -> &'static str {
    match source {
        VersionSource::Given => "given",
        VersionSource::BuildkiteArtifacts => "from Buildkite artifacts",
        VersionSource::DebianRepository => "recovered from the Debian repository",
    }
}

fn mark(presence: &Presence) -> ColoredString {
    match presence {
        Presence::Present => "[ok]     ".green(),
        Presence::Missing => "[missing]".red(),
        Presence::Unknown(_) => "[unknown]".yellow(),
    }
}

fn colour_state(state: &str) -> ColoredString {
    match state {
        "passed" => state.green(),
        "failed" | "canceled" => state.red(),
        "running" | "scheduled" => state.cyan(),
        _ => state.normal(),
    }
}

pub fn summarise_totals(totals: &Totals) -> String {
    if totals.total() == 0 {
        return "nothing checked".to_string();
    }
    let mut parts = vec![format!("{}/{} present", totals.present, totals.total())];
    if totals.missing > 0 {
        parts.push(format!("{} missing", totals.missing));
    }
    if totals.unknown > 0 {
        parts.push(format!("{} unknown", totals.unknown));
    }
    parts.join(", ")
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let kept: String = value.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_report_unknown_separately_from_missing() {
        let totals = Totals {
            present: 2,
            missing: 1,
            unknown: 3,
        };
        let text = summarise_totals(&totals);
        assert!(text.contains("2/6 present"));
        assert!(text.contains("1 missing"));
        assert!(text.contains("3 unknown"));
    }

    #[test]
    fn nothing_checked_is_said_plainly() {
        assert_eq!(summarise_totals(&Totals::default()), "nothing checked");
    }

    #[test]
    fn long_values_are_truncated() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
