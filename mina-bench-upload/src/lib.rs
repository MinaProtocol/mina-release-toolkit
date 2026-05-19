//! Parse Mina benchmark output and upload to InfluxDB.
//!
//! This crate replaces the legacy Python uploader at
//! `mina/scripts/benchmarks/` and deliberately drops one responsibility:
//! it no longer **executes** the benchmark binary. The caller (a
//! dhall/bash CI step) is responsible for running the benchmark and
//! piping its stdout into this tool's stdin (or writing it to a file
//! passed via `--input`).
//!
//! Architecture:
//!
//!   * [`parse`] — one module per benchmark output format. Each parser
//!     implements [`parse::Parser`] and returns `Vec<BenchmarkRecord>`.
//!   * [`influx`] — write (line-protocol upload via `influxdb2`) and
//!     query (Flux for regression checks).
//!   * [`regression`] — compare current values against the moving
//!     average of the last N samples for the same branch+measurement+field.
//!   * [`config`] — env-var validation at startup, not at upload time.

pub mod config;
pub mod influx;
pub mod parse;
pub mod regression;

pub use parse::{BenchmarkRecord, FieldValue, Parser};
