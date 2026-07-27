//! Generic passthrough parser: `--format generic-json`.
//!
//! Unlike the format-specific parsers, this one imposes no benchmark
//! shape. A project that can emit its own metrics as JSON gets upload and
//! regression checks without anyone writing a bespoke parser for it, which
//! is what makes this tool reusable outside mina.
//!
//! Input is a JSON array of records, each mapping 1:1 to an InfluxDB
//! point:
//!
//! ```json
//! [
//!   {
//!     "measurement": "archive_memory_bench",
//!     "tags": { "variant": "ci" },
//!     "fields": { "pg_backend_rss_peak_kib": 161624, "blocks_per_sec": 20.87 },
//!     "timestamp_ns": 1784999223801092376
//!   }
//! ]
//! ```
//!
//!   * `measurement` (string, required).
//!   * `tags` (object of string→string, optional). `gitbranch` is always
//!     set from `--branch` and takes precedence over any tag of the same
//!     name, since the regression query filters on it.
//!   * `fields` (object of string→number, required and non-empty). An
//!     integral JSON number becomes an InfluxDB integer field, otherwise
//!     a float.
//!   * `timestamp_ns` (integer, optional). Omitted → the server stamps
//!     it on receipt.

use super::{BenchmarkRecord, FieldValue, Parser, TAG_GITBRANCH};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

pub struct GenericJsonParser;

#[derive(Debug, Deserialize)]
struct RawRecord {
    measurement: String,
    #[serde(default)]
    tags: BTreeMap<String, String>,
    fields: BTreeMap<String, serde_json::Number>,
    #[serde(default)]
    timestamp_ns: Option<i64>,
}

impl Parser for GenericJsonParser {
    fn parse(&self, input: &str, branch: &str) -> Result<Vec<BenchmarkRecord>> {
        let raws: Vec<RawRecord> = serde_json::from_str(input).context(
            "generic-json: expected a JSON array of {measurement, tags, fields} records",
        )?;

        raws.into_iter()
            .map(|raw| {
                if raw.fields.is_empty() {
                    return Err(anyhow!(
                        "generic-json: record '{}' has no fields",
                        raw.measurement
                    ));
                }
                let mut rec = BenchmarkRecord::new(raw.measurement);
                for (k, v) in raw.tags {
                    rec = rec.with_tag(k, v);
                }
                // Set gitbranch last so it can't be shadowed by a tag of
                // the same name in the input — the regression query keys on it.
                rec = rec.with_tag(TAG_GITBRANCH, branch);
                for (k, n) in raw.fields {
                    rec = rec.with_field(k, number_to_field(&n));
                }
                rec.timestamp_ns = raw.timestamp_ns;
                Ok(rec)
            })
            .collect()
    }
}

/// Preserve integer-ness: an integral JSON number becomes an InfluxDB
/// integer field (`123i`), anything else a float. Mixing the two types
/// for the same field across uploads is an InfluxDB error, so callers
/// should stay consistent per field.
fn number_to_field(n: &serde_json::Number) -> FieldValue {
    if let Some(i) = n.as_i64() {
        FieldValue::Int(i)
    } else {
        FieldValue::Float(n.as_f64().unwrap_or(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::TAG_GITBRANCH;

    const FIXTURE: &str = include_str!("../../tests/fixtures/generic.json");

    #[test]
    fn parses_all_records() {
        let records = GenericJsonParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn measurement_tags_and_fields_carry_through() {
        let records = GenericJsonParser.parse(FIXTURE, "develop").unwrap();
        let r = &records[0];
        assert_eq!(r.measurement, "archive_memory_bench");
        assert_eq!(r.tags.get("variant").map(String::as_str), Some("ci"));
        assert!(r.fields.contains_key("pg_backend_rss_peak_kib"));
    }

    #[test]
    fn branch_becomes_gitbranch_tag() {
        let records = GenericJsonParser.parse(FIXTURE, "my-branch").unwrap();
        assert_eq!(
            records[0].tags.get(TAG_GITBRANCH).map(String::as_str),
            Some("my-branch")
        );
    }

    #[test]
    fn gitbranch_from_cli_overrides_a_tag_of_the_same_name() {
        let input = r#"[{"measurement":"m","tags":{"gitbranch":"stale"},"fields":{"x":1}}]"#;
        let records = GenericJsonParser.parse(input, "authoritative").unwrap();
        assert_eq!(
            records[0].tags.get(TAG_GITBRANCH).map(String::as_str),
            Some("authoritative")
        );
    }

    #[test]
    fn integral_numbers_stay_integers() {
        let input = r#"[{"measurement":"m","fields":{"count":7,"ratio":0.5}}]"#;
        let records = GenericJsonParser.parse(input, "develop").unwrap();
        assert_eq!(records[0].fields.get("count"), Some(&FieldValue::Int(7)));
        assert_eq!(
            records[0].fields.get("ratio"),
            Some(&FieldValue::Float(0.5))
        );
    }

    #[test]
    fn optional_timestamp_is_read() {
        let records = GenericJsonParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records[0].timestamp_ns, Some(1784999223801092376));
        assert_eq!(records[1].timestamp_ns, None);
    }

    #[test]
    fn record_with_no_fields_errors() {
        let input = r#"[{"measurement":"m","fields":{}}]"#;
        let err = GenericJsonParser.parse(input, "develop").unwrap_err();
        assert!(err.to_string().contains("no fields"));
    }

    #[test]
    fn malformed_json_errors() {
        let err = GenericJsonParser.parse("not json", "develop").unwrap_err();
        assert!(err.to_string().contains("generic-json"));
    }

    #[test]
    fn empty_array_is_valid() {
        let records = GenericJsonParser.parse("[]", "develop").unwrap();
        assert!(records.is_empty());
    }
}
