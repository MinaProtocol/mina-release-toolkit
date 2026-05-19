//! Parser for the `transaction-snark-profiler` markdown table.
//!
//! Input shape:
//! ```text
//! | No.| Proof updates| Non-proof pairs| Non-proof singles| Mempool verification time (sec)| Transaction proving time (sec)|Permutation|
//! |--|--|--|--|--|--|--|
//! | 1| 0| 1| 1| 0.002070| 12.125372| SSS|
//! ```
//!
//! Measurement name is the *permutation* column (the trailing
//! all-letters cell, e.g. `SSS`). The five numeric columns become
//! fields. Row-number column (`No.`) is discarded.

use super::{BenchmarkRecord, FieldValue, Parser, TAG_CATEGORY, TAG_GITBRANCH};
use anyhow::{anyhow, Context, Result};

pub const F_PROOFS_UPDATES: &str = "proofs updates";
pub const F_NON_PROOF_PAIRS: &str = "non-proof pairs";
pub const F_NON_PROOF_SINGLES: &str = "non-proof singles";
pub const F_VERIFICATION_TIME: &str = "verification time";
pub const F_PROVING_TIME: &str = "value";

pub struct SnarkParser;

impl Parser for SnarkParser {
    fn category(&self) -> &'static str {
        "snark"
    }

    fn parse(&self, input: &str, branch: &str) -> Result<Vec<BenchmarkRecord>> {
        let mut out = Vec::new();

        for line in input.lines() {
            // Pipe-delimited rows. The separator row (---) starts with
            // `|` too, but every cell is `--`-only; the column header
            // row also starts with `|`. We filter both out below.
            if !line.starts_with('|') {
                continue;
            }
            if line.contains("--") {
                continue;
            }

            let cells: Vec<&str> = line
                .split('|')
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect();

            if cells.is_empty() {
                continue;
            }
            // Header has the literal "No." in the first cell.
            if cells[0].eq_ignore_ascii_case("no.") {
                continue;
            }
            if cells.len() < 7 {
                continue;
            }

            // Layout: No | Proofs | NPpairs | NPsingles | VerifTime | ProvTime | Permutation
            let proofs_updates: f64 = cells[1]
                .parse()
                .with_context(|| format!("parsing proofs updates on line: {}", line))?;
            let np_pairs: f64 = cells[2]
                .parse()
                .with_context(|| format!("parsing non-proof pairs on line: {}", line))?;
            let np_singles: f64 = cells[3]
                .parse()
                .with_context(|| format!("parsing non-proof singles on line: {}", line))?;
            let verif_time: f64 = cells[4]
                .parse()
                .with_context(|| format!("parsing verification time on line: {}", line))?;
            let prov_time: f64 = cells[5]
                .parse()
                .with_context(|| format!("parsing proving time on line: {}", line))?;
            let name = cells[6].to_string();
            if name.is_empty() {
                return Err(anyhow!("snark: empty permutation on line {}", line));
            }

            out.push(
                BenchmarkRecord::new(name)
                    .with_tag(TAG_CATEGORY, "snark")
                    .with_tag(TAG_GITBRANCH, branch)
                    .with_field(F_PROOFS_UPDATES, FieldValue::Float(proofs_updates))
                    .with_field(F_NON_PROOF_PAIRS, FieldValue::Float(np_pairs))
                    .with_field(F_NON_PROOF_SINGLES, FieldValue::Float(np_singles))
                    .with_field(F_VERIFICATION_TIME, FieldValue::Float(verif_time))
                    .with_field(F_PROVING_TIME, FieldValue::Float(prov_time)),
            );
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/snark.txt");

    #[test]
    fn parses_four_rows() {
        let records = SnarkParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records.len(), 4);
    }

    #[test]
    fn measurement_is_the_permutation() {
        let records = SnarkParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records[0].measurement, "SSS");
        assert_eq!(records[3].measurement, "SPP");
    }

    #[test]
    fn proving_time_lands_in_value_field() {
        let records = SnarkParser.parse(FIXTURE, "develop").unwrap();
        let v = records[0].fields.get(F_PROVING_TIME).unwrap().as_f64();
        assert!((v - 12.125372).abs() < 1e-9, "got {}", v);
    }

    #[test]
    fn preamble_and_separator_rows_are_skipped() {
        let records = SnarkParser.parse(FIXTURE, "develop").unwrap();
        assert!(records.iter().all(|r| !r.measurement.contains("--")));
        assert!(records
            .iter()
            .all(|r| !r.measurement.eq_ignore_ascii_case("no.")));
    }

    #[test]
    fn category_tag_is_snark() {
        let records = SnarkParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(
            records[0].tags.get(TAG_CATEGORY).map(String::as_str),
            Some("snark")
        );
    }
}
