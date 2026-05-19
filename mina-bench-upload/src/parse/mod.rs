//! Format parsers. Every benchmark output type lives in its own
//! submodule, implements the [`Parser`] trait, and returns a
//! [`BenchmarkRecord`] vector that the upload + regression-check
//! paths can consume uniformly.

use anyhow::Result;
use std::collections::BTreeMap;

pub mod archive;
pub mod heap;
pub mod janestreet;
pub mod ledger_apply;
pub mod snark;
pub mod zkapp;

/// A single row of benchmark output ready for InfluxDB.
///
/// The struct mirrors InfluxDB's line-protocol shape (measurement +
/// tags + fields + optional timestamp) so a `BenchmarkRecord` is a
/// 1:1 mapping to one `_measurement` row in the bucket.
///
/// Field and tag *names* (the `BTreeMap` keys) must match what the
/// Python tool was uploading historically — any rename here breaks
/// regression checks against existing samples. See each parser's
/// `tag_names` / `field_names` constants for the authoritative list.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkRecord {
    pub measurement: String,
    pub tags: BTreeMap<String, String>,
    pub fields: BTreeMap<String, FieldValue>,
    /// Unix nanoseconds. `None` lets the InfluxDB server stamp it on receipt.
    pub timestamp_ns: Option<i64>,
}

impl BenchmarkRecord {
    pub fn new(measurement: impl Into<String>) -> Self {
        Self {
            measurement: measurement.into(),
            tags: BTreeMap::new(),
            fields: BTreeMap::new(),
            timestamp_ns: None,
        }
    }

    /// Construct a record pre-populated with the two tags every parser
    /// has to set: `category` (the benchmark family) and `gitbranch`
    /// (the source git branch). Both are required by the regression
    /// query's Flux filter, so building them in saves seven copies of
    /// the same two-tag boilerplate across the parser modules.
    pub fn categorized(measurement: impl Into<String>, category: &str, branch: &str) -> Self {
        Self::new(measurement)
            .with_tag(TAG_CATEGORY, category)
            .with_tag(TAG_GITBRANCH, branch)
    }

    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags.insert(key.into(), value.into());
        self
    }

    pub fn with_field(mut self, key: impl Into<String>, value: FieldValue) -> Self {
        self.fields.insert(key.into(), value);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldValue {
    Float(f64),
    Int(i64),
}

impl FieldValue {
    pub fn as_f64(&self) -> f64 {
        match self {
            FieldValue::Float(v) => *v,
            FieldValue::Int(v) => *v as f64,
        }
    }
}

impl From<f64> for FieldValue {
    fn from(v: f64) -> Self {
        FieldValue::Float(v)
    }
}

impl From<i64> for FieldValue {
    fn from(v: i64) -> Self {
        FieldValue::Int(v)
    }
}

/// A benchmark-output parser. Implementations are stateless and
/// project `&str` of raw benchmark stdout into a list of records.
///
/// The trait deliberately has just one method. Each parser embeds
/// its own `category` tag value when constructing records (see
/// [`BenchmarkRecord::categorized`]); exposing it as a separate
/// trait method would split that knowledge across two places.
pub trait Parser {
    /// Parse `input` into records. `branch` is attached as the
    /// `gitbranch` tag on every record. Returns an empty `Vec` if no
    /// rows could be extracted (caller can decide whether that's an
    /// error).
    fn parse(&self, input: &str, branch: &str) -> Result<Vec<BenchmarkRecord>>;
}

/// Tag keys used by every parser. Centralized so renames are
/// blast-radius-aware.
pub const TAG_CATEGORY: &str = "category";
pub const TAG_GITBRANCH: &str = "gitbranch";
