//! Mutations on the Debian control file: read a field, set/replace a field,
//! and bulk-update dependency version constraints during a reversion.

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::Path;

/// Read the value of `field` from the RFC822 control file at `path`.
/// Multi-line continuations (lines starting with whitespace) are concatenated
/// with a single space, matching the way Debian tools render the field.
pub fn read_field(path: &Path, field: &str) -> Result<String> {
    let text = fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?;
    let prefix = format!("{}:", field);
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.starts_with(&prefix) {
            let mut value = line[prefix.len()..].trim().to_string();
            // Collect continuations.
            for cont in lines.by_ref() {
                if cont.starts_with(|c: char| c.is_whitespace()) {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(cont.trim());
                } else {
                    break;
                }
            }
            return Ok(value);
        }
    }
    Err(anyhow!("Field {} not found in {}", field, path.display()))
}

/// Set `field` to `value` in the control file at `path`. If the field
/// already exists, every line of its (potentially multi-line) value is
/// replaced with a single-line `Field: value`. If it doesn't exist, the
/// field is appended at the end of the file (before any trailing blank
/// line) — matching the bash `replace-suite` script's behaviour.
pub fn set_field(path: &Path, field: &str, value: &str) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?;
    let prefix = format!("{}:", field);

    let mut out_lines: Vec<String> = Vec::new();
    let mut replaced = false;
    let mut skipping_continuation = false;

    for line in text.lines() {
        if skipping_continuation {
            if line.starts_with(|c: char| c.is_whitespace()) {
                continue; // continuation of the field we just removed
            }
            skipping_continuation = false;
        }
        if !replaced && line.starts_with(&prefix) {
            out_lines.push(format!("{}: {}", field, value));
            replaced = true;
            skipping_continuation = true;
            continue;
        }
        out_lines.push(line.to_string());
    }

    if !replaced {
        // Append. Strip trailing blank lines, append, then add one trailing newline.
        while out_lines.last().is_some_and(|l| l.trim().is_empty()) {
            out_lines.pop();
        }
        out_lines.push(format!("{}: {}", field, value));
    }

    let mut new_text = out_lines.join("\n");
    new_text.push('\n');
    fs::write(path, new_text).with_context(|| format!("Writing {}", path.display()))?;
    Ok(())
}

/// Rewrite every dependency version constraint that pins exactly the
/// `old_version` to instead reference `new_version`. Operates on the
/// `Depends`, `Pre-Depends`, `Recommends`, `Suggests`, `Enhances`,
/// `Breaks`, `Conflicts`, `Replaces`, and `Provides` fields. Matches the
/// `--update-deps` flag in `deb-session-reversion.sh`.
pub fn update_deps(path: &Path, old_version: &str, new_version: &str) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?;

    // Operators recognized in a Debian version constraint: =, <<, <=, >=, >>
    let dep_fields = [
        "Depends",
        "Pre-Depends",
        "Recommends",
        "Suggests",
        "Enhances",
        "Breaks",
        "Conflicts",
        "Replaces",
        "Provides",
    ];

    let mut out = String::with_capacity(text.len());
    let mut in_dep_field = false;
    for line in text.lines() {
        let mut current = line.to_string();
        // First column of a new field — decide whether we're now inside
        // a dependency field.
        if !line.starts_with(|c: char| c.is_whitespace()) {
            in_dep_field = false;
            if let Some((name, _)) = line.split_once(':') {
                if dep_fields.contains(&name.trim()) {
                    in_dep_field = true;
                }
            }
        }
        if in_dep_field {
            current = replace_version_in_constraints(&current, old_version, new_version);
        }
        out.push_str(&current);
        out.push('\n');
    }

    fs::write(path, out).with_context(|| format!("Writing {}", path.display()))?;
    Ok(())
}

/// Replace occurrences of `(<op> <old_version>)` with `(<op> <new_version>)`
/// inside a single dependency line. Anything not enclosed in `(…)` is left
/// alone (so package names that happen to contain a version suffix aren't
/// mangled).
fn replace_version_in_constraints(line: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '(' {
            out.push(c);
            let mut paren = String::new();
            for c in chars.by_ref() {
                if c == ')' {
                    let replaced = replace_in_constraint(&paren, old, new);
                    out.push_str(&replaced);
                    out.push(')');
                    break;
                }
                paren.push(c);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn replace_in_constraint(inside: &str, old: &str, new: &str) -> String {
    // Only exact `=` pins are rewritten — loose constraints like `>=` are
    // still satisfied by the bumped version and intentionally left alone.
    let trimmed = inside.trim();
    let Some((op, rest)) = trimmed.split_once(char::is_whitespace) else {
        return inside.to_string();
    };
    let op = op.trim();
    let ver = rest.trim();
    if op != "=" {
        return inside.to_string();
    }
    if ver != old {
        return inside.to_string();
    }
    format!("{} {}", op, new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(text: &str) -> tempfile::NamedTempFile {
        let f = tempfile::NamedTempFile::new().unwrap();
        fs::write(f.path(), text).unwrap();
        f
    }

    #[test]
    fn read_field_simple() {
        let f = write_tmp("Package: foo\nVersion: 1.0\n");
        assert_eq!(read_field(f.path(), "Package").unwrap(), "foo");
        assert_eq!(read_field(f.path(), "Version").unwrap(), "1.0");
    }

    #[test]
    fn read_field_missing() {
        let f = write_tmp("Package: foo\n");
        assert!(read_field(f.path(), "Suite").is_err());
    }

    #[test]
    fn read_field_multiline() {
        let f = write_tmp("Package: foo\nDescription: short\n line 2\n line 3\n");
        assert_eq!(
            read_field(f.path(), "Description").unwrap(),
            "short line 2 line 3"
        );
    }

    #[test]
    fn set_field_replaces_existing() {
        let f = write_tmp("Package: foo\nVersion: 1.0\nArchitecture: amd64\n");
        set_field(f.path(), "Version", "2.0").unwrap();
        let out = fs::read_to_string(f.path()).unwrap();
        assert!(out.contains("Version: 2.0\n"));
        assert!(!out.contains("Version: 1.0"));
        assert!(out.contains("Architecture: amd64"));
    }

    #[test]
    fn set_field_appends_if_missing() {
        let f = write_tmp("Package: foo\nVersion: 1.0\n");
        set_field(f.path(), "Suite", "stable").unwrap();
        let out = fs::read_to_string(f.path()).unwrap();
        assert!(out.contains("Suite: stable"));
    }

    #[test]
    fn set_field_strips_multiline_value() {
        let f = write_tmp(
            "Package: foo\nDescription: short\n longer body\n more body\nArchitecture: amd64\n",
        );
        set_field(f.path(), "Description", "replaced").unwrap();
        let out = fs::read_to_string(f.path()).unwrap();
        assert!(out.contains("Description: replaced"));
        assert!(!out.contains("longer body"));
        assert!(out.contains("Architecture: amd64"));
    }

    #[test]
    fn update_deps_rewrites_equality_only() {
        let f = write_tmp(
            "Package: foo\nVersion: 2.0\nDepends: libfoo (= 1.0), libbar (>= 1.0), libbaz\n",
        );
        update_deps(f.path(), "1.0", "2.0").unwrap();
        let out = fs::read_to_string(f.path()).unwrap();
        assert!(out.contains("libfoo (= 2.0)"), "got: {}", out);
        // Loose constraint must NOT be rewritten — `>= 1.0` is still satisfied by 2.0.
        assert!(out.contains("libbar (>= 1.0)"), "got: {}", out);
        assert!(out.contains("libbaz"));
    }

    #[test]
    fn update_deps_ignores_non_dep_fields() {
        let f =
            write_tmp("Package: foo\nDescription: needs version 1.0\nDepends: libfoo (= 1.0)\n");
        update_deps(f.path(), "1.0", "2.0").unwrap();
        let out = fs::read_to_string(f.path()).unwrap();
        // Description must stay untouched
        assert!(out.contains("needs version 1.0"));
        // Depends must be rewritten
        assert!(out.contains("libfoo (= 2.0)"));
    }

    #[test]
    fn update_deps_leaves_unrelated_versions_alone() {
        let f = write_tmp("Depends: libfoo (= 1.0.5), libbar (= 1.0)\n");
        update_deps(f.path(), "1.0", "2.0").unwrap();
        let out = fs::read_to_string(f.path()).unwrap();
        assert!(out.contains("libfoo (= 1.0.5)"));
        assert!(out.contains("libbar (= 2.0)"));
    }
}
