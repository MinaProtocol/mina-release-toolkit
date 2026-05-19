//! Flux query for fetching recent samples of a (branch, measurement,
//! field) tuple. Powers the regression check.

use crate::config::InfluxConfig;
use anyhow::{Context, Result};
use influxdb2::Client;
use influxdb2_structmap::value::Value;
use influxdb2_structmap::{FromMap, GenericMap};

/// One historical observation pulled from InfluxDB.
///
/// The `influxdb2` crate's query API needs a `FromMap` impl on the
/// row type; the structmap-derive crate is unmaintained, so we
/// implement the trait by hand. The shape is minimal: a single
/// `_value` column projected by the Flux `keep(columns:["_value"])`
/// pipeline below.
#[derive(Debug, Default)]
pub struct Sample {
    pub value: f64,
}

impl FromMap for Sample {
    fn from_genericmap(map: GenericMap) -> Self {
        let value = match map.get("_value") {
            Some(Value::Double(v)) => v.into_inner(),
            Some(Value::Long(v)) => *v as f64,
            Some(Value::UnsignedLong(v)) => *v as f64,
            _ => 0.0,
        };
        Self { value }
    }
}

/// Aggregate stats over the last N samples for one
/// (branch, measurement, field) triple.
#[derive(Debug, Clone)]
pub struct HistoricalMean {
    pub samples_found: usize,
    pub mean: f64,
}

impl HistoricalMean {
    pub fn from_samples(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let sum: f64 = values.iter().sum();
        Some(Self {
            samples_found: values.len(),
            mean: sum / values.len() as f64,
        })
    }
}

/// Fetch up to `n` most-recent values of `field` recorded against
/// the given `branch` + `measurement` tuple, then compute their mean.
/// Returns `None` when zero samples exist (signals "no history" to
/// the caller, which decides whether to skip or fail).
pub async fn historical_mean(
    cfg: &InfluxConfig,
    branch: &str,
    measurement: &str,
    field: &str,
    n: usize,
) -> Result<Option<HistoricalMean>> {
    let q = format!(
        "from(bucket: \"{bucket}\")
           |> range(start: -30d)
           |> filter(fn: (r) => r[\"gitbranch\"] == \"{branch}\"
                                and r._measurement == \"{measurement}\"
                                and r._field == \"{field}\")
           |> keep(columns: [\"_value\", \"_time\"])
           |> sort(columns: [\"_time\"], desc: true)
           |> limit(n: {n})",
        bucket = cfg.bucket,
        branch = escape(branch),
        measurement = escape(measurement),
        field = escape(field),
        n = n,
    );

    let client = Client::new(&cfg.host, &cfg.org, &cfg.token);
    let rows: Vec<Sample> = client
        .query::<Sample>(Some(influxdb2::models::Query::new(q.clone())))
        .await
        .with_context(|| format!("InfluxDB query failed:\n{}", q))?;

    let values: Vec<f64> = rows.into_iter().map(|s| s.value).collect();
    Ok(HistoricalMean::from_samples(&values))
}

/// Escape Flux string literal embedded inside double quotes. Real-world
/// branch / measurement names can contain `\`, `"`, and other Flux
/// metacharacters; we only need to escape the two that break the quote.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_samples_empty_is_none() {
        assert!(HistoricalMean::from_samples(&[]).is_none());
    }

    #[test]
    fn from_samples_computes_arithmetic_mean() {
        let h = HistoricalMean::from_samples(&[10.0, 20.0, 30.0]).unwrap();
        assert_eq!(h.samples_found, 3);
        assert!((h.mean - 20.0).abs() < 1e-9);
    }

    #[test]
    fn escape_doubles_backslashes_and_escapes_quotes() {
        assert_eq!(escape(r#"a\b"c"#), r#"a\\b\"c"#);
    }
}
