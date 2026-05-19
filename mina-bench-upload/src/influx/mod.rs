//! InfluxDB v2 client wrapper.
//!
//! Wraps the `influxdb2` crate so callers see a small surface (write,
//! historical mean query) and so the underlying client can be swapped
//! for tests without touching the parser or CLI layers.

pub mod query;
pub mod write;

pub use query::HistoricalMean;
pub use write::upload;
