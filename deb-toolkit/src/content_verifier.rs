//! Port of `src/lib/content_verifier.ml`, with four bug fixes vs the OCaml
//! original:
//!
//! 1. `Dpkg_Deb_Output.from_str` read `package_name` and `version` from the
//!    `"Architecture"` field. We read them from `Package:` / `Version:`.
//! 2. `check_required_property` / `check_optional_property` returned the
//!    error branch on equality (inverted). We return errors on **inequality**.
//! 3. `Scanf.sscanf description "Built from %s by %s"` is brittle and only
//!    parses when both fields are whitespace-free tokens. Replaced with a
//!    regex.
//! 4. `String.split ~on:':'` of each field line failed for values containing
//!    `:` (URLs in `Homepage`/`Buildurl`). Replaced with `splitn(2, ':')`.

use anyhow::{anyhow, Result};
use regex::Regex;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

use crate::cli::{OptionalMetadataArgs, VerifyContentArgs};
use crate::defaults::Defaults;
use crate::misc::check_file_exists;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpkgDebOutput {
    pub package_name: String,
    pub version: String,
    pub architecture: String,
    pub vendor: Option<String>,
    pub authors: Option<String>,
    pub maintainer: Option<String>,
    pub description: Option<String>,
    pub section: Option<String>,
    pub priority: Option<String>,
    pub homepage: Option<String>,
    pub installed_size: Option<String>,
    pub source: Option<String>,
    pub suite: Option<String>,
    pub codename: Option<String>,
    pub license: Option<String>,
    pub githash: Option<String>,
    pub buildurl: Option<String>,
    pub depends: Vec<String>,
    pub suggested_depends: Vec<String>,
    pub recommended_depends: Vec<String>,
    pub pre_depends: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    pub provides: Vec<String>,
}

/// Parse the output of `dpkg-deb -I`.
///
/// dpkg-deb -I prints a small preamble ("new Debian package, version 2.0.",
/// "size … bytes: …", a control-archive line) followed by the package's
/// control file contents indented by one space per line.
///
/// We walk every line, strip a single leading space if present, treat lines
/// that match `^[A-Za-z0-9-]+:` as new field headers, and treat any line that
/// starts with whitespace after the strip as a continuation of the previous
/// field's value (with the leading space removed).
pub fn parse_dpkg_deb_output(text: &str) -> Result<DpkgDebOutput> {
    let field_re = Regex::new(r"^([A-Za-z0-9-]+):\s?(.*)$").unwrap();

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut current_name: Option<String> = None;

    for raw_line in text.lines() {
        // dpkg-deb -I prefixes every control-file line with one space. Strip it.
        let line = raw_line.strip_prefix(' ').unwrap_or(raw_line);

        // A new field starts at column 0 (after the dpkg-deb space-strip) and
        // matches "Name: value".
        if !line.starts_with(char::is_whitespace) {
            if let Some(caps) = field_re.captures(line) {
                let name = caps.get(1).unwrap().as_str().to_string();
                let value = caps.get(2).unwrap().as_str().to_string();
                fields.insert(name.clone(), value);
                current_name = Some(name);
                continue;
            }
            // Lines like "new Debian package, version 2.0." don't match.
            // Reset the current field — they aren't continuations.
            current_name = None;
            continue;
        }

        // Continuation line — append to the most recent field's value.
        if let Some(name) = &current_name {
            if let Some(existing) = fields.get_mut(name) {
                let stripped = line.trim_start();
                existing.push('\n');
                existing.push_str(stripped);
            }
        }
    }

    let take = |name: &str| fields.get(name).map(|s| s.trim().to_string());
    let split_list = |name: &str| -> Vec<String> {
        match fields.get(name) {
            None => Vec::new(),
            Some(v) => v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        }
    };

    let package_name = take("Package").ok_or_else(|| anyhow!("Package field missing"))?;
    let version = take("Version").ok_or_else(|| anyhow!("Version field missing"))?;
    let architecture = take("Architecture").ok_or_else(|| anyhow!("Architecture field missing"))?;

    // Description is multi-line. The OCaml original looked for a trailer
    // "Built from <githash> by <buildurl>" embedded in the body.
    let raw_description = fields.get("Description").cloned();
    let (description, githash, buildurl) = match &raw_description {
        None => (None, None, None),
        Some(body) => {
            let built_re = Regex::new(r"(?m)^\s*Built from (\S+) by (\S+)\s*$").unwrap();
            let (gh, bu) = match built_re.captures(body) {
                Some(c) => (
                    Some(c.get(1).unwrap().as_str().to_string()),
                    Some(c.get(2).unwrap().as_str().to_string()),
                ),
                None => (None, None),
            };
            // First non-empty line is the human description.
            let summary = body
                .lines()
                .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("Built from"))
                .map(|l| l.trim().to_string());
            (summary, gh, bu)
        }
    };

    Ok(DpkgDebOutput {
        package_name,
        version,
        architecture,
        vendor: take("Vendor"),
        authors: take("Authors"),
        maintainer: take("Maintainer"),
        description,
        section: take("Section"),
        priority: take("Priority"),
        homepage: take("Homepage"),
        installed_size: take("Installed-Size"),
        source: take("Source"),
        suite: take("Suite"),
        codename: take("Codename"),
        license: take("License"),
        githash,
        buildurl,
        depends: split_list("Depends"),
        suggested_depends: split_list("Suggests"),
        recommended_depends: split_list("Recommends"),
        pre_depends: split_list("Pre-Depends"),
        conflicts: split_list("Conflicts"),
        replaces: split_list("Replaces"),
        provides: split_list("Provides"),
    })
}

fn get_deb_output(deb: &str, debug: bool) -> Result<DpkgDebOutput> {
    if debug {
        log::info!("Executing: dpkg-deb -I {}", deb);
    }
    let output = Command::new("dpkg-deb")
        .args(["-I", deb])
        .output()
        .map_err(|e| anyhow!("Failed to spawn dpkg-deb: {}", e))?;
    if !output.status.success() {
        anyhow::bail!(
            "dpkg-deb -I {} failed: {}",
            deb,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_dpkg_deb_output(&text)
}

/// Required field: must equal the expected value.
fn check_required(expected: &str, actual: &str, name: &str) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(anyhow!(
            "{} mismatch. Expected: {}, Actual: {}",
            name,
            expected,
            actual
        ))
    }
}

/// Optional field: if expected is provided, actual must match.
fn check_optional(expected: Option<&str>, actual: Option<&str>, name: &str) -> Result<()> {
    match (expected, actual) {
        (None, _) => Ok(()),
        (Some(_), None) => Err(anyhow!(
            "{} mismatch. Expected: {}, Actual: None",
            name,
            expected.unwrap()
        )),
        (Some(e), Some(a)) if e == a => Ok(()),
        (Some(e), Some(a)) => Err(anyhow!("{} mismatch. Expected: {}, Actual: {}", name, e, a)),
    }
}

/// List field: every entry in `expected` must appear in `actual`.
fn check_list(expected: &Option<Vec<String>>, actual: &[String], name: &str) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual_set: BTreeSet<&str> = actual.iter().map(|s| s.as_str()).collect();
    let missing: Vec<&str> = expected
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !actual_set.contains(s))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(anyhow!("{} mismatch. Missing: {}", name, missing.join(",")))
    }
}

fn split_csv(s: &Option<String>) -> Option<Vec<String>> {
    s.as_ref()
        .map(|raw| raw.split(',').map(|s| s.to_string()).collect())
}

fn merge_list(cli: Option<Vec<String>>, defaults: Option<Vec<String>>) -> Option<Vec<String>> {
    match (cli, defaults) {
        (Some(a), Some(b)) => {
            let mut out = a;
            out.extend(b);
            Some(out)
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn first<T: Clone>(cli: Option<T>, default: Option<T>) -> Option<T> {
    cli.or(default)
}

pub fn verify(args: &VerifyContentArgs) -> Result<()> {
    check_file_exists(&args.deb)?;

    let defaults = Defaults::load(args.defaults_file.as_deref())?;
    let m: &OptionalMetadataArgs = &args.metadata;

    let depends = merge_list(split_csv(&m.depends), defaults.depends);
    let suggested_depends = merge_list(split_csv(&m.suggested_depends), defaults.suggested_depends);
    let recommended_depends = merge_list(
        split_csv(&m.recommended_depends),
        defaults.recommended_depends,
    );
    let pre_depends = merge_list(split_csv(&m.pre_depends), defaults.pre_depends);
    let conflicts = merge_list(split_csv(&m.conflicts), defaults.conflicts);
    let replaces = merge_list(split_csv(&m.replaces), defaults.replaces);
    let provides = merge_list(split_csv(&m.provides), defaults.provides);

    let vendor = first(m.vendor.clone(), defaults.vendor);
    let authors = first(m.authors.clone(), defaults.package_authors);
    let maintainer = first(m.maintainer.clone(), defaults.package_maintainer);
    let description = first(m.description.clone(), defaults.package_description);
    let section = first(m.section.clone(), defaults.package_section);
    let priority = first(m.priority.clone(), defaults.package_priority);
    let homepage = first(m.homepage.clone(), defaults.package_homepage);
    let installed_size = first(m.installed_size.clone(), defaults.package_installed_size);
    let source = first(m.source.clone(), defaults.package_source);
    let architecture = first(m.architecture.clone(), defaults.architecture);
    let license = first(m.license.clone(), defaults.license);
    let githash = first(m.githash.clone(), defaults.githash);
    let buildurl = first(m.buildurl.clone(), defaults.buildurl);

    let deb_output = get_deb_output(&args.deb, args.debug)?;
    log::info!(
        "deb_output: {}",
        serde_json::to_string(&debug_repr(&deb_output)).unwrap_or_default()
    );

    // Package + Version are derived from the .deb filename (mina convention:
    // `<package>_<version>.deb`) and cross-checked against control fields.
    let filename = Path::new(&args.deb)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("Cannot derive filename from {}", args.deb))?;
    let stem = filename.strip_suffix(".deb").unwrap_or(filename);
    let (expected_pkg, expected_ver) = stem.split_once('_').ok_or_else(|| {
        anyhow!(
            "Filename {} is not in <package>_<version>.deb form",
            filename
        )
    })?;

    check_required(expected_pkg, &deb_output.package_name, "Package")?;
    check_required(expected_ver, &deb_output.version, "Version")?;

    check_optional(
        architecture.as_deref(),
        Some(deb_output.architecture.as_str()),
        "Architecture",
    )?;
    check_optional(vendor.as_deref(), deb_output.vendor.as_deref(), "Vendor")?;
    check_optional(authors.as_deref(), deb_output.authors.as_deref(), "Authors")?;
    check_optional(
        maintainer.as_deref(),
        deb_output.maintainer.as_deref(),
        "Maintainer",
    )?;
    check_optional(
        description.as_deref(),
        deb_output.description.as_deref(),
        "Description",
    )?;
    check_optional(section.as_deref(), deb_output.section.as_deref(), "Section")?;
    check_optional(
        priority.as_deref(),
        deb_output.priority.as_deref(),
        "Priority",
    )?;
    check_optional(
        homepage.as_deref(),
        deb_output.homepage.as_deref(),
        "Homepage",
    )?;
    check_optional(
        installed_size.as_deref(),
        deb_output.installed_size.as_deref(),
        "Installed-Size",
    )?;
    check_optional(source.as_deref(), deb_output.source.as_deref(), "Source")?;
    check_optional(license.as_deref(), deb_output.license.as_deref(), "License")?;
    check_optional(githash.as_deref(), deb_output.githash.as_deref(), "Githash")?;
    check_optional(
        buildurl.as_deref(),
        deb_output.buildurl.as_deref(),
        "Buildurl",
    )?;
    check_optional(args.suite.as_deref(), deb_output.suite.as_deref(), "Suite")?;
    check_optional(
        args.codename.as_deref(),
        deb_output.codename.as_deref(),
        "Codename",
    )?;

    check_list(&depends, &deb_output.depends, "Depends")?;
    check_list(
        &suggested_depends,
        &deb_output.suggested_depends,
        "Suggests",
    )?;
    check_list(
        &recommended_depends,
        &deb_output.recommended_depends,
        "Recommends",
    )?;
    check_list(&pre_depends, &deb_output.pre_depends, "Pre-Depends")?;
    check_list(&conflicts, &deb_output.conflicts, "Conflicts")?;
    check_list(&replaces, &deb_output.replaces, "Replaces")?;
    check_list(&provides, &deb_output.provides, "Provides")?;

    Ok(())
}

fn debug_repr(o: &DpkgDebOutput) -> serde_json::Value {
    // Lightweight, dep-free JSON view for the log line (we don't want to
    // add serde derives to the public type just for this).
    serde_json::json!({
        "package_name": o.package_name,
        "version": o.version,
        "architecture": o.architecture,
        "vendor": o.vendor,
        "authors": o.authors,
        "maintainer": o.maintainer,
        "description": o.description,
        "section": o.section,
        "priority": o.priority,
        "homepage": o.homepage,
        "installed_size": o.installed_size,
        "source": o.source,
        "suite": o.suite,
        "codename": o.codename,
        "license": o.license,
        "githash": o.githash,
        "buildurl": o.buildurl,
        "depends": o.depends,
        "suggested_depends": o.suggested_depends,
        "recommended_depends": o.recommended_depends,
        "pre_depends": o.pre_depends,
        "conflicts": o.conflicts,
        "replaces": o.replaces,
        "provides": o.provides,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DPKG_DEB_I: &str = r#" new Debian package, version 2.0.
 size 123456 bytes: control archive=789 bytes.
     200 bytes,     12 lines      control
 Package: example-app
 Version: 1.0.0
 Architecture: amd64
 Maintainer: O(1)Labs <build@o1labs.org>
 Section: base
 Priority: optional
 Homepage: https://minaprotocol.com/
 Installed-Size: 100
 Depends: libc6 (>= 2.17), zlib1g
 Suggests: foo, bar
 License: Apache-2.0
 Description: example app
  Built from fakehash by https://buildkite.com/x/y
"#;

    #[test]
    fn parses_basic_fields() {
        let out = parse_dpkg_deb_output(SAMPLE_DPKG_DEB_I).expect("parse");
        assert_eq!(out.package_name, "example-app");
        assert_eq!(out.version, "1.0.0");
        assert_eq!(out.architecture, "amd64");
        assert_eq!(out.section.as_deref(), Some("base"));
        assert_eq!(out.priority.as_deref(), Some("optional"));
        assert_eq!(out.installed_size.as_deref(), Some("100"));
    }

    #[test]
    fn url_field_with_colon_is_preserved() {
        // Bug #4 in OCaml: split on ':' broke for values containing ':'.
        let out = parse_dpkg_deb_output(SAMPLE_DPKG_DEB_I).expect("parse");
        assert_eq!(out.homepage.as_deref(), Some("https://minaprotocol.com/"));
    }

    #[test]
    fn description_parses_built_from_trailer() {
        // Bug #3: OCaml's Scanf "%s by %s" tokenization was brittle.
        let out = parse_dpkg_deb_output(SAMPLE_DPKG_DEB_I).expect("parse");
        assert_eq!(out.description.as_deref(), Some("example app"));
        assert_eq!(out.githash.as_deref(), Some("fakehash"));
        assert_eq!(out.buildurl.as_deref(), Some("https://buildkite.com/x/y"));
    }

    #[test]
    fn depends_split_into_list() {
        let out = parse_dpkg_deb_output(SAMPLE_DPKG_DEB_I).expect("parse");
        assert_eq!(out.depends, vec!["libc6 (>= 2.17)", "zlib1g"]);
        assert_eq!(out.suggested_depends, vec!["foo", "bar"]);
    }

    #[test]
    fn package_name_and_version_from_correct_fields() {
        // Bug #1: OCaml read package_name and version from "Architecture".
        let out = parse_dpkg_deb_output(SAMPLE_DPKG_DEB_I).expect("parse");
        assert_eq!(out.package_name, "example-app");
        assert_ne!(out.package_name, out.architecture);
        assert_eq!(out.version, "1.0.0");
        assert_ne!(out.version, out.architecture);
    }

    #[test]
    fn missing_required_field_errors() {
        let txt = " Package: foo\n Version: 1.0\n";
        let err = parse_dpkg_deb_output(txt).unwrap_err();
        assert!(err.to_string().contains("Architecture"), "got: {}", err);
    }

    #[test]
    fn check_required_matches() {
        assert!(check_required("foo", "foo", "Package").is_ok());
    }

    #[test]
    fn check_required_mismatches() {
        // Bug #2: OCaml returned Ok on inequality, error on equality.
        let err = check_required("foo", "bar", "Package").unwrap_err();
        assert!(err.to_string().contains("Package mismatch"));
    }

    #[test]
    fn check_optional_none_expected_is_ok() {
        assert!(check_optional(None, Some("anything"), "X").is_ok());
        assert!(check_optional(None, None, "X").is_ok());
    }

    #[test]
    fn check_optional_actual_none_errors() {
        let err = check_optional(Some("foo"), None, "X").unwrap_err();
        assert!(err.to_string().contains("Actual: None"));
    }

    #[test]
    fn check_optional_matches() {
        assert!(check_optional(Some("foo"), Some("foo"), "X").is_ok());
    }

    #[test]
    fn check_optional_mismatches() {
        let err = check_optional(Some("foo"), Some("bar"), "X").unwrap_err();
        assert!(err.to_string().contains("Expected: foo, Actual: bar"));
    }

    #[test]
    fn check_list_subset_passes() {
        let exp = Some(vec!["a".to_string(), "b".to_string()]);
        let act = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(check_list(&exp, &act, "Depends").is_ok());
    }

    #[test]
    fn check_list_missing_item_errors() {
        let exp = Some(vec!["a".to_string(), "b".to_string()]);
        let act = vec!["a".to_string()];
        let err = check_list(&exp, &act, "Depends").unwrap_err();
        assert!(err.to_string().contains("Missing: b"));
    }

    #[test]
    fn check_list_none_expected_passes() {
        let exp: Option<Vec<String>> = None;
        let act = vec!["a".to_string()];
        assert!(check_list(&exp, &act, "Depends").is_ok());
    }
}
