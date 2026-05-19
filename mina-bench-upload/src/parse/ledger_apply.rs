//! Parser for the ledger-apply benchmark JSON.
//!
//! Input is a single JSON object:
//!
//! ```json
//! { "final_time": "0.4000", "preparation_steps_mean": "0.432" }
//! ```
//!
//! Note both values are JSON **strings** carrying numeric content in
//! the existing pipeline (the OCaml benchmark serializes them that
//! way). We parse them as `f64`.
//!
//! The resulting record always has the constant measurement name
//! `"ledger-apply"` — there's only one row per invocation. Fields are
//! `time` and `preps mean`, matching the Python tool's column names.

use super::{BenchmarkRecord, FieldValue, Parser, TAG_CATEGORY, TAG_GITBRANCH};
use anyhow::{Context, Result};
use serde::Deserialize;

pub const F_TIME: &str = "time";
pub const F_PREPS_MEAN: &str = "preps mean";
pub const MEASUREMENT: &str = "ledger-apply";

pub struct LedgerApplyParser;

#[derive(Debug, Deserialize)]
struct Raw {
    final_time: String,
    preparation_steps_mean: String,
}

impl Parser for LedgerApplyParser {
    fn category(&self) -> &'static str {
        "ledger-apply"
    }

    fn parse(&self, input: &str, branch: &str) -> Result<Vec<BenchmarkRecord>> {
        let raw: Raw =
            serde_json::from_str(input).context("ledger-apply: not a valid JSON object")?;
        let final_time: f64 = raw.final_time.parse().with_context(|| {
            format!(
                "ledger-apply: final_time={:?} is not a float",
                raw.final_time
            )
        })?;
        let preps_mean: f64 = raw.preparation_steps_mean.parse().with_context(|| {
            format!(
                "ledger-apply: preparation_steps_mean={:?} is not a float",
                raw.preparation_steps_mean
            )
        })?;

        Ok(vec![BenchmarkRecord::new(MEASUREMENT)
            .with_tag(TAG_CATEGORY, "ledger-apply")
            .with_tag(TAG_GITBRANCH, branch)
            .with_field(F_TIME, FieldValue::Float(final_time))
            .with_field(F_PREPS_MEAN, FieldValue::Float(preps_mean))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/ledger_apply.json");

    #[test]
    fn parses_single_row() {
        let records = LedgerApplyParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].measurement, "ledger-apply");
    }

    #[test]
    fn fields_are_parsed_from_string_values() {
        let records = LedgerApplyParser.parse(FIXTURE, "develop").unwrap();
        let t = records[0].fields.get(F_TIME).unwrap().as_f64();
        let p = records[0].fields.get(F_PREPS_MEAN).unwrap().as_f64();
        assert!((t - 0.4000).abs() < 1e-9);
        assert!((p - 0.432).abs() < 1e-9);
    }

    #[test]
    fn malformed_json_errors() {
        let err = LedgerApplyParser.parse("{}", "develop").unwrap_err();
        assert!(err.to_string().contains("ledger-apply"));
    }
}
