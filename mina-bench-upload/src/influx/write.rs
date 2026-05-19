//! Upload [`BenchmarkRecord`]s to InfluxDB v2 via the `influxdb2`
//! crate. Builds `DataPoint`s on the fly and streams them to the
//! `/api/v2/write` endpoint. No subprocesses, no CSV, no
//! kill-on-204 workarounds — just an HTTP write.

use crate::config::InfluxConfig;
use crate::parse::{BenchmarkRecord, FieldValue};
use anyhow::{Context, Result};
use futures::stream;
use influxdb2::models::DataPoint;
use influxdb2::Client;

/// Push `records` to the configured bucket. Returns the number of
/// records actually sent. Caller is responsible for `--dry-run`
/// short-circuiting; this function always hits the network.
pub async fn upload(records: &[BenchmarkRecord], cfg: &InfluxConfig) -> Result<usize> {
    if records.is_empty() {
        return Ok(0);
    }
    let client = Client::new(&cfg.host, &cfg.org, &cfg.token);
    let points = records
        .iter()
        .map(record_to_data_point)
        .collect::<Result<Vec<_>>>()?;
    let n = points.len();
    client
        .write(&cfg.bucket, stream::iter(points))
        .await
        .with_context(|| format!("InfluxDB write to {}/{} failed", cfg.host, cfg.bucket))?;
    Ok(n)
}

fn record_to_data_point(r: &BenchmarkRecord) -> Result<DataPoint> {
    let mut builder = DataPoint::builder(&r.measurement);
    for (k, v) in &r.tags {
        builder = builder.tag(k, v);
    }
    for (k, v) in &r.fields {
        builder = match v {
            FieldValue::Float(f) => builder.field(k, *f),
            FieldValue::Int(i) => builder.field(k, *i),
        };
    }
    if let Some(ts) = r.timestamp_ns {
        builder = builder.timestamp(ts);
    }
    builder
        .build()
        .with_context(|| format!("building DataPoint for measurement {}", r.measurement))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{TAG_CATEGORY, TAG_GITBRANCH};

    #[test]
    fn empty_records_short_circuits() {
        // upload() returns Ok(0) without touching the network when
        // there's nothing to send.
        let cfg = InfluxConfig {
            host: "http://127.0.0.1:1".into(),
            token: "x".into(),
            org: "x".into(),
            bucket: "x".into(),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let n = rt.block_on(upload(&[], &cfg)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn record_to_data_point_carries_tags_and_fields() {
        let r = BenchmarkRecord::new("m")
            .with_tag(TAG_CATEGORY, "zkapp")
            .with_tag(TAG_GITBRANCH, "develop")
            .with_field("cost", FieldValue::Float(1.0))
            .with_field("count", FieldValue::Int(7));
        let _dp = record_to_data_point(&r).unwrap();
        // We can't introspect DataPoint fields (the crate keeps them
        // private), but failure to build would have errored above.
    }
}
