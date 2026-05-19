//! Parser for `mina-zkapp-limits` output.
//!
//! Each input line is of the form:
//!
//! ```text
//! Proofs updates=0  Signed/None updates=0  Pairs of Signed/None updates=1: Total account updates: 2 Cost: 10.080000
//! ```
//!
//! The measurement name is computed as `P{p}S{s}PS{ps}TA{ta}` (the
//! same composite the Python tool uses, kept verbatim to preserve
//! historical InfluxDB measurement names).

use super::{BenchmarkRecord, FieldValue, Parser, TAG_CATEGORY, TAG_GITBRANCH};
use anyhow::Result;
use regex::Regex;

pub const F_PROOFS_UPDATES: &str = "proofs updates";
pub const F_SIGNED_UPDATES: &str = "signed updates";
pub const F_PAIRS_OF_SIGNED: &str = "pairs of signed";
pub const F_TOTAL_ACCOUNT_UPDATES: &str = "total account updates";
pub const F_COST: &str = "cost";

pub struct ZkappParser;

impl Parser for ZkappParser {
    fn category(&self) -> &'static str {
        "zkapp"
    }

    fn parse(&self, input: &str, branch: &str) -> Result<Vec<BenchmarkRecord>> {
        let re = Regex::new(
            r"Proofs updates=(?P<p>\d+)  Signed/None updates=(?P<s>\d+)  Pairs of Signed/None updates=(?P<ps>\d+): Total account updates: (?P<ta>\d+) Cost: (?P<cost>[0-9]*\.?[0-9]+)",
        )
        .unwrap();

        let mut out = Vec::new();
        for line in input.lines() {
            let Some(caps) = re.captures(line) else {
                continue;
            };
            let p: i64 = caps["p"].parse()?;
            let s: i64 = caps["s"].parse()?;
            let ps: i64 = caps["ps"].parse()?;
            let ta: i64 = caps["ta"].parse()?;
            let cost: f64 = caps["cost"].parse()?;
            let name = format!("P{}S{}PS{}TA{}", p, s, ps, ta);

            out.push(
                BenchmarkRecord::new(name)
                    .with_tag(TAG_CATEGORY, "zkapp")
                    .with_tag(TAG_GITBRANCH, branch)
                    .with_field(F_PROOFS_UPDATES, FieldValue::Int(p))
                    .with_field(F_SIGNED_UPDATES, FieldValue::Int(s))
                    .with_field(F_PAIRS_OF_SIGNED, FieldValue::Int(ps))
                    .with_field(F_TOTAL_ACCOUNT_UPDATES, FieldValue::Int(ta))
                    .with_field(F_COST, FieldValue::Float(cost)),
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/zkapp.txt");

    #[test]
    fn parses_five_rows() {
        let records = ZkappParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records.len(), 5);
    }

    #[test]
    fn measurement_uses_composite_naming() {
        let records = ZkappParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records[0].measurement, "P0S0PS1TA2");
        assert_eq!(records[2].measurement, "P0S1PS0TA1");
        assert_eq!(records[4].measurement, "P1S0PS0TA1");
    }

    #[test]
    fn cost_is_a_float() {
        let records = ZkappParser.parse(FIXTURE, "develop").unwrap();
        let v = records[0].fields.get(F_COST).unwrap().as_f64();
        assert!((v - 10.080000).abs() < 1e-9);
    }

    #[test]
    fn non_matching_lines_are_skipped() {
        let input = "garbage\n\
                     Proofs updates=1  Signed/None updates=2  Pairs of Signed/None updates=3: Total account updates: 4 Cost: 5.0\n\
                     more garbage\n";
        let records = ZkappParser.parse(input, "develop").unwrap();
        assert_eq!(records.len(), 1);
    }
}
