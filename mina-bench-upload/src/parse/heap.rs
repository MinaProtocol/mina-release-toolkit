//! Parser for `mina-heap-usage` output.
//!
//! Lines look like:
//!
//! ```text
//! Data of type Zkapp_command.t                                uses  52268 heap words =   418144 bytes
//! ```
//!
//! The Python tool collapses all whitespace from the line before
//! parsing, so a type with spaces in its name (e.g.
//! `Dummy Pickles.Side_loaded.Proof.t`) gets stored in InfluxDB as
//! `DummyPickles.Side_loaded.Proof.t`. We replicate that mangling so
//! the measurement names match what's already in the bucket.

use super::{BenchmarkRecord, FieldValue, Parser, TAG_CATEGORY, TAG_GITBRANCH};
use anyhow::{anyhow, Context, Result};
use regex::Regex;

pub const F_HEAP_WORDS: &str = "heap words";
pub const F_BYTES: &str = "bytes";

pub struct HeapParser;

impl Parser for HeapParser {
    fn category(&self) -> &'static str {
        // Python uses underscore here (`heap_usage`), not the
        // CLI-friendly `heap-usage`. Match it for compat.
        "heap_usage"
    }

    fn parse(&self, input: &str, branch: &str) -> Result<Vec<BenchmarkRecord>> {
        // After whitespace-stripping, the line shape is exactly:
        //     Dataoftype<NAME>uses<WORDS>heapwords=<BYTES>bytes
        let re =
            Regex::new(r"^Dataoftype(?P<name>.+?)uses(?P<words>\d+)heapwords=(?P<bytes>\d+)bytes$")
                .unwrap();

        let mut out = Vec::new();
        for line in input.lines() {
            if !line.trim_start().starts_with("Data of type") {
                continue;
            }
            let collapsed: String = line.chars().filter(|c| !c.is_whitespace()).collect();

            let caps = re
                .captures(&collapsed)
                .ok_or_else(|| anyhow!("heap: line did not match shape: {}", line))?;
            let name = caps["name"].to_string();
            let words: f64 = caps["words"]
                .parse()
                .with_context(|| format!("heap: bad heap-words on line {}", line))?;
            let bytes: f64 = caps["bytes"]
                .parse()
                .with_context(|| format!("heap: bad bytes on line {}", line))?;

            out.push(
                BenchmarkRecord::new(name)
                    .with_tag(TAG_CATEGORY, "heap_usage")
                    .with_tag(TAG_GITBRANCH, branch)
                    .with_field(F_HEAP_WORDS, FieldValue::Float(words))
                    .with_field(F_BYTES, FieldValue::Float(bytes)),
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/heap.txt");

    #[test]
    fn parses_four_rows() {
        let records = HeapParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records.len(), 4);
    }

    #[test]
    fn name_has_whitespace_stripped() {
        let records = HeapParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(records[0].measurement, "Zkapp_command.t");
        // Multi-word name gets glued together — preserves Python-compat
        // measurement naming.
        assert_eq!(records[3].measurement, "DummyPickles.Side_loaded.Proof.t");
    }

    #[test]
    fn heap_words_and_bytes_fields() {
        let records = HeapParser.parse(FIXTURE, "develop").unwrap();
        let w = records[0].fields.get(F_HEAP_WORDS).unwrap().as_f64();
        let b = records[0].fields.get(F_BYTES).unwrap().as_f64();
        assert_eq!(w as i64, 52268);
        assert_eq!(b as i64, 418144);
    }

    #[test]
    fn category_tag_is_underscored() {
        let records = HeapParser.parse(FIXTURE, "develop").unwrap();
        assert_eq!(
            records[0].tags.get(TAG_CATEGORY).map(String::as_str),
            Some("heap_usage")
        );
    }

    #[test]
    fn lines_not_starting_with_data_of_type_are_skipped() {
        let input = "preamble\n\
                     Data of type Foo.t uses 1 heap words = 8 bytes\n\
                     trailer\n";
        let records = HeapParser.parse(input, "develop").unwrap();
        assert_eq!(records.len(), 1);
    }
}
