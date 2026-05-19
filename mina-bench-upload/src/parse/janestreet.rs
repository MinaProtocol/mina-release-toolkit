//! Parser for `core_bench`-style JaneStreet tables.
//!
//! Used by `mina-base` and `ledger-export`. The output is a
//! pipe-delimited markdown-style table where every data row starts
//! with `│`, e.g.:
//!
//! ```text
//! │ Name                            │ Time/Run │ Cycls/Run │ mWd/Run │ mjWd/Run │ Prom/Run │
//! ├─────────────────────────────────┼──────────┼───────────┼─────────┼──────────┼──────────┤
//! │ [mina_base.foo] some_test       │  12.34us │    45.6kc │   1.00w │    0.00w │   0.00w  │
//! ```
//!
//! Normalization (matches the Python tool's behaviour bit-for-bit so
//! historical InfluxDB samples stay queryable by the same `_field`
//! names):
//!
//!   * Name: strip any `[...]` segments and trim.
//!   * Time/Run: `12.34us` → `12.34`. `500.0ns` → `0.5` (us). Anything
//!     else is a parse error.
//!   * Cycls/Run: `45.6kc` → `45.6`. `10000.0c` → `10000.0`.
//!   * mWd/Run, mjWd/Run, Prom/Run: strip trailing `w`.

use super::{BenchmarkRecord, FieldValue, Parser};
use anyhow::{anyhow, Context, Result};
use regex::Regex;

/// Field names — must match what the Python tool wrote to InfluxDB so
/// regression queries against historical samples keep resolving.
pub const F_TIME_PER_RUN: &str = "Time/Run [us]";
pub const F_CYCLES_PER_RUN: &str = "Cycls/Run [kc]";
pub const F_MINOR_WORDS_PER_RUN: &str = "mWd/Run [w]";
pub const F_MAJOR_WORDS_PER_RUN: &str = "mjWd/Run [w]";
pub const F_PROMOTIONS_PER_RUN: &str = "Prom/Run [w]";

pub struct JaneStreetParser {
    /// `mina-base` or `ledger-export`. Becomes the `category` tag.
    category: &'static str,
}

impl JaneStreetParser {
    pub fn mina_base() -> Self {
        Self {
            category: "mina-base",
        }
    }

    pub fn ledger_export() -> Self {
        Self {
            category: "ledger-export",
        }
    }
}

impl Parser for JaneStreetParser {
    fn parse(&self, input: &str, branch: &str) -> Result<Vec<BenchmarkRecord>> {
        let mut out = Vec::new();
        let bracket_re = Regex::new(r"\[.*?\]").unwrap();

        for line in input.lines() {
            // Data rows start with the box-drawing `│`. The header
            // row also matches but its first cell is literally
            // "Name" so we skip it explicitly.
            if !line.starts_with('│') {
                continue;
            }

            let cells: Vec<&str> = line
                .split('│')
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect();

            // Expected: name + 5 metric columns.
            if cells.len() < 6 {
                continue;
            }
            if cells[0].starts_with("Name") {
                continue;
            }

            let raw_name = cells[0];
            let name = bracket_re.replace_all(raw_name, "").trim().to_string();
            if name.is_empty() {
                return Err(anyhow!(
                    "janestreet: empty name after [...]-stripping on line: {}",
                    line
                ));
            }

            let time_us =
                parse_time_us(cells[1]).with_context(|| format!("parsing Time/Run on {}", line))?;
            let cycles_kc = parse_cycles_kc(cells[2])
                .with_context(|| format!("parsing Cycls/Run on {}", line))?;
            let minor_w =
                parse_words(cells[3]).with_context(|| format!("parsing mWd/Run on {}", line))?;
            let major_w =
                parse_words(cells[4]).with_context(|| format!("parsing mjWd/Run on {}", line))?;
            let prom_w =
                parse_words(cells[5]).with_context(|| format!("parsing Prom/Run on {}", line))?;

            out.push(
                BenchmarkRecord::categorized(name, self.category, branch)
                    .with_field(F_TIME_PER_RUN, FieldValue::Float(time_us))
                    .with_field(F_CYCLES_PER_RUN, FieldValue::Float(cycles_kc))
                    .with_field(F_MINOR_WORDS_PER_RUN, FieldValue::Float(minor_w))
                    .with_field(F_MAJOR_WORDS_PER_RUN, FieldValue::Float(major_w))
                    .with_field(F_PROMOTIONS_PER_RUN, FieldValue::Float(prom_w)),
            );
        }

        Ok(out)
    }
}

/// `12.34us` → `12.34`. `500.0ns` → `0.5` (converted to us). Any other
/// suffix is a parse error — matches the Python tool which raises
/// "Time can be expressed only in us or ns".
fn parse_time_us(cell: &str) -> Result<f64> {
    let s = cell.trim();
    if let Some(stripped) = s.strip_suffix("us") {
        Ok(stripped.parse()?)
    } else if let Some(stripped) = s.strip_suffix("ns") {
        let ns: f64 = stripped.parse()?;
        Ok(ns / 1_000.0)
    } else {
        Err(anyhow!(
            "Time can be expressed only in us or ns, got {:?}",
            s
        ))
    }
}

/// `45.6kc` → `45.6`. `10000.0c` → `10000.0`. Field is reported as
/// kilocycles in InfluxDB but the Python tool keeps the displayed
/// number verbatim regardless of which suffix was on the cell, so
/// we do the same.
fn parse_cycles_kc(cell: &str) -> Result<f64> {
    let s = cell.trim();
    if let Some(stripped) = s.strip_suffix("kc") {
        Ok(stripped.parse()?)
    } else if let Some(stripped) = s.strip_suffix('c') {
        Ok(stripped.parse()?)
    } else {
        Err(anyhow!("Cycles cell must end in 'kc' or 'c', got {:?}", s))
    }
}

/// `1.00w` → `1.0`. Strip the trailing `w` unit marker.
fn parse_words(cell: &str) -> Result<f64> {
    let s = cell.trim();
    let stripped = s
        .strip_suffix('w')
        .ok_or_else(|| anyhow!("Word cell must end in 'w', got {:?}", s))?;
    Ok(stripped.parse()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{TAG_CATEGORY, TAG_GITBRANCH};

    const FIXTURE: &str = include_str!("../../tests/fixtures/janestreet.txt");

    #[test]
    fn parses_three_rows() {
        let p = JaneStreetParser::mina_base();
        let records = p.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records.len(), 3);
    }

    #[test]
    fn strips_bracketed_name_prefix() {
        let p = JaneStreetParser::mina_base();
        let records = p.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records[0].measurement, "some_test");
        assert_eq!(records[1].measurement, "another_test");
    }

    #[test]
    fn time_in_us_is_kept_as_is() {
        let p = JaneStreetParser::mina_base();
        let records = p.parse(FIXTURE, "develop").unwrap();
        let v = records[0].fields.get(F_TIME_PER_RUN).unwrap().as_f64();
        assert!((v - 12.34).abs() < 1e-9, "got {}", v);
    }

    #[test]
    fn time_in_ns_is_converted_to_us() {
        let p = JaneStreetParser::mina_base();
        let records = p.parse(FIXTURE, "develop").unwrap();
        // 500.0ns / 1000 == 0.5 us
        let v = records[1].fields.get(F_TIME_PER_RUN).unwrap().as_f64();
        assert!((v - 0.5).abs() < 1e-9, "got {}", v);
    }

    #[test]
    fn plain_c_suffix_is_accepted() {
        let p = JaneStreetParser::mina_base();
        let records = p.parse(FIXTURE, "develop").unwrap();
        let v = records[2].fields.get(F_CYCLES_PER_RUN).unwrap().as_f64();
        assert!((v - 10_000.0).abs() < 1e-9, "got {}", v);
    }

    #[test]
    fn tags_carry_category_and_branch() {
        let p = JaneStreetParser::mina_base();
        let records = p.parse(FIXTURE, "feature-x").unwrap();
        let r = &records[0];
        assert_eq!(
            r.tags.get(TAG_CATEGORY).map(String::as_str),
            Some("mina-base")
        );
        assert_eq!(
            r.tags.get(TAG_GITBRANCH).map(String::as_str),
            Some("feature-x")
        );
    }

    #[test]
    fn ledger_export_variant_changes_only_category() {
        let p = JaneStreetParser::ledger_export();
        let records = p.parse(FIXTURE, "develop").unwrap();
        assert_eq!(
            records[0].tags.get(TAG_CATEGORY).map(String::as_str),
            Some("ledger-export")
        );
    }

    #[test]
    fn header_row_is_skipped() {
        let p = JaneStreetParser::mina_base();
        let records = p.parse(FIXTURE, "develop").unwrap();
        // 3 data rows, never the header (which starts with "Name").
        assert!(records.iter().all(|r| r.measurement != "Name"));
    }
}
