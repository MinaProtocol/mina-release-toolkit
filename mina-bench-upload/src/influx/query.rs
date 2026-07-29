//! Flux query for fetching recent samples of a (branch, measurement,
//! field) tuple. Powers the regression check.

use crate::config::InfluxConfig;
use anyhow::{Context, Result};
use influxdb2::Client;
use influxdb2_structmap::value::Value;

/// Read a numeric InfluxDB `_value` cell as `f64`, regardless of the
/// integer/float type it deserialized into. Non-numeric or absent cells
/// yield `None`, so they drop out of the sample set rather than skewing
/// the mean with a zero.
fn value_as_f64(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Double(v)) => Some(v.into_inner()),
        Some(Value::Long(v)) => Some(*v as f64),
        Some(Value::UnsignedLong(v)) => Some(*v as f64),
        _ => None,
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
    // Every interpolated string is escaped — even `bucket`, which
    // comes from a trusted env var. Treat the Flux query as a
    // boundary and never bypass escaping; if we ever take a bucket
    // from CLI input or a config file we don't want to discover this
    // is a regression.
    let q = format!(
        "from(bucket: \"{bucket}\")
           |> range(start: -30d)
           |> filter(fn: (r) => r[\"gitbranch\"] == \"{branch}\"
                                and r._measurement == \"{measurement}\"
                                and r._field == \"{field}\")
           |> keep(columns: [\"_value\", \"_time\"])
           |> sort(columns: [\"_time\"], desc: true)
           |> limit(n: {n})",
        bucket = escape(&cfg.bucket),
        branch = escape(branch),
        measurement = escape(measurement),
        field = escape(field),
        n = n,
    );

    // Use `query_raw` rather than the typed `query::<T>` helper: the
    // latter pivots every row on a `_field` column and panics
    // (`Option::unwrap()` on `None`) when the projection doesn't include
    // one -- which ours doesn't, since we `keep` only `_value`/`_time`.
    // `query_raw` hands back the rows untouched and is unfazed by an
    // empty result set.
    let client = Client::new(&cfg.host, &cfg.org, &cfg.token);
    let rows = client
        .query_raw(Some(influxdb2::models::Query::new(q.clone())))
        .await
        .with_context(|| format!("InfluxDB query failed:\n{}", q))?;

    let values: Vec<f64> = rows
        .iter()
        .filter_map(|r| value_as_f64(r.values.get("_value")))
        .collect();
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
