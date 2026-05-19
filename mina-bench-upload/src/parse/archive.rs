//! Parser for the archive benchmark JSON.
//!
//! Input is a JSON array of `{operation: String, avg_time_ms: f64}`.
//! The operation name becomes the InfluxDB measurement; `avg_time_ms`
//! lands in the `time` field (matching the Python tool's field name).

use super::{BenchmarkRecord, FieldValue, Parser};
use anyhow::{Context, Result};
use serde::Deserialize;

pub const F_TIME: &str = "time";

pub struct ArchiveParser;

#[derive(Debug, Deserialize)]
struct ArchiveRow {
    operation: String,
    avg_time_ms: f64,
}

impl Parser for ArchiveParser {
    fn parse(&self, input: &str, branch: &str) -> Result<Vec<BenchmarkRecord>> {
        let rows: Vec<ArchiveRow> =
            serde_json::from_str(input).context("archive: not a valid JSON array")?;

        Ok(rows
            .into_iter()
            .map(|r| {
                BenchmarkRecord::categorized(r.operation, "archive", branch)
                    .with_field(F_TIME, FieldValue::Float(r.avg_time_ms))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/archive.json");

    #[test]
    fn parses_three_rows() {
        let records = ArchiveParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records.len(), 3);
    }

    #[test]
    fn measurement_is_the_operation_name() {
        let records = ArchiveParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records[0].measurement, "Zkapp_account_update.add");
        assert_eq!(records[2].measurement, "Block.add");
    }

    #[test]
    fn time_field_is_avg_time_ms() {
        let records = ArchiveParser.parse(FIXTURE, "develop").unwrap();
        let v = records[2].fields.get(F_TIME).unwrap().as_f64();
        assert!((v - 12.345).abs() < 1e-9);
    }

    #[test]
    fn empty_array_is_valid() {
        let records = ArchiveParser.parse("[]", "develop").unwrap();
        assert_eq!(records.len(), 0);
    }

    #[test]
    fn malformed_json_errors() {
        let err = ArchiveParser.parse("not json", "develop").unwrap_err();
        assert!(err.to_string().contains("archive"));
    }
}
