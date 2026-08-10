//! Regression check.
//!
//! Compare a freshly-parsed value against the moving average of the
//! last `N` samples for the same `(branch, measurement, field)` triple.
//! Two thresholds, both expressed as **fractions of the mean**:
//!
//!   * `yellow` (default `0.1`, i.e. 10% over mean) — warning, no
//!     non-zero exit.
//!   * `red`    (default `0.2`, i.e. 20% over mean) — fail the build.
//!
//! This fixes a long-standing bug in the Python implementation
//! (`bench.py:137`, `isclose(value + red_threshold, average)`), where
//! the comparison was both inverted and unit-less — adding a percent
//! to an absolute value and then checking equality, not exceedance.
//! The result: production regressions never tripped the `red`
//! threshold. The correct comparison, as implemented here, is
//! `value > mean * (1 + threshold)`.
//!
//! Behaviour when fewer than `N` historical samples exist: emit
//! [`CheckOutcome::NotEnoughHistory`] and let the caller decide. We
//! default to "skip" in the CLI so a brand-new metric doesn't block
//! the first deploy.

use crate::config::InfluxConfig;
use crate::influx::query::historical_mean;
use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum CheckOutcome {
    /// Current value is within the yellow threshold of the historical
    /// mean. All good.
    Ok { current: f64, mean: f64 },
    /// Current value exceeds `mean * (1 + yellow)` but is below
    /// `mean * (1 + red)`. Warn, do not fail.
    Yellow {
        current: f64,
        mean: f64,
        ceiling: f64,
    },
    /// Current value exceeds `mean * (1 + red)`. Fail the build.
    Red {
        current: f64,
        mean: f64,
        ceiling: f64,
    },
    /// Fewer than `min_samples` historical points exist; we can't
    /// trustworthily check yet.
    NotEnoughHistory {
        samples_found: usize,
        required: usize,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Fraction of mean above which we warn. e.g. `0.10` = +10%.
    pub yellow: f64,
    /// Fraction of mean above which we fail. e.g. `0.20` = +20%.
    pub red: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            yellow: 0.10,
            red: 0.20,
        }
    }
}

/// Which thresholds apply to which metric.
///
/// One pair of thresholds cannot fit a benchmark whose fields differ in
/// run-to-run variance. The snark bench measures both on every run: over
/// four consecutive nightlies of 17 permutations, the `value` field
/// varied by a median of 1.0% run to run, while `verification time`
/// varied by 30% (max 87%). At the default `red = 0.20`, `value` never
/// tripped in 68 samples — a good gate — and `verification time` tripped
/// 24% of them, none of which was a regression: the median
/// `current / mean` was 1.06, i.e. sitting on the historical mean. With
/// 17 permutations checked independently that is a 99% chance of failing
/// the build every night on noise alone.
///
/// Raising the global `red` to cover the noisy field would drop the
/// useful gate on the quiet one, so the threshold has to be chosen per
/// metric instead.
///
/// A key is matched whole, first as `"<measurement>.<field>"` and then as
/// `"<field>"`; it is never split on `.`, so measurement names that
/// contain dots (`Zkapp_account_update.add`) behave predictably. Matching
/// the bare field name is what makes this practical for the snark bench,
/// where one key covers all 17 permutations.
#[derive(Debug, Clone, Default)]
pub struct ThresholdTable {
    default: Thresholds,
    overrides: HashMap<String, Thresholds>,
    excluded: HashSet<String>,
}

impl ThresholdTable {
    /// A table that applies `default` to every metric.
    pub fn new(default: Thresholds) -> Self {
        Self {
            default,
            overrides: HashMap::new(),
            excluded: HashSet::new(),
        }
    }

    /// Use `thresholds` for metrics matching `key`.
    pub fn with_override(mut self, key: impl Into<String>, thresholds: Thresholds) -> Self {
        self.overrides.insert(key.into(), thresholds);
        self
    }

    /// Never gate the build on metrics matching `key`. They are still
    /// parsed and uploaded, so the trend stays visible in InfluxDB.
    pub fn with_exclusion(mut self, key: impl Into<String>) -> Self {
        self.excluded.insert(key.into());
        self
    }

    /// The thresholds to apply, or `None` when this metric is excluded
    /// from the gate.
    pub fn resolve(&self, measurement: &str, field: &str) -> Option<Thresholds> {
        let qualified = format!("{}.{}", measurement, field);
        if self.excluded.contains(&qualified) || self.excluded.contains(field) {
            return None;
        }
        Some(
            self.overrides
                .get(&qualified)
                .or_else(|| self.overrides.get(field))
                .copied()
                .unwrap_or(self.default),
        )
    }
}

/// Parse a `--field-threshold` value: `<key>=<yellow>,<red>`.
///
/// Split on the LAST `=` so a key may contain one, and reject
/// `yellow > red`, which would make the yellow band unreachable and is
/// far more likely a typo than an intent.
pub fn parse_field_threshold(spec: &str) -> Result<(String, Thresholds)> {
    let (key, values) = spec
        .rsplit_once('=')
        .ok_or_else(|| anyhow!("expected <key>=<yellow>,<red>, got {:?}", spec))?;
    let key = key.trim();
    if key.is_empty() {
        return Err(anyhow!("empty metric key in {:?}", spec));
    }
    let (yellow, red) = values
        .split_once(',')
        .ok_or_else(|| anyhow!("expected <yellow>,<red> after '=' in {:?}", spec))?;
    let parse = |s: &str, which: &str| -> Result<f64> {
        let v: f64 = s
            .trim()
            .parse()
            .map_err(|_| anyhow!("{} threshold in {:?} is not a number", which, spec))?;
        if !v.is_finite() || v < 0.0 {
            return Err(anyhow!(
                "{} threshold in {:?} must be a non-negative fraction",
                which,
                spec
            ));
        }
        Ok(v)
    };
    let thresholds = Thresholds {
        yellow: parse(yellow, "yellow")?,
        red: parse(red, "red")?,
    };
    if thresholds.yellow > thresholds.red {
        return Err(anyhow!(
            "yellow ({}) is above red ({}) in {:?}",
            thresholds.yellow,
            thresholds.red,
            spec
        ));
    }
    Ok((key.to_string(), thresholds))
}

/// Pure comparison — no I/O. Exposed so tests don't need to mock
/// InfluxDB.
pub fn evaluate(
    current: f64,
    mean: Option<f64>,
    samples_found: usize,
    min_samples: usize,
    thresholds: Thresholds,
) -> CheckOutcome {
    // Skip when we don't have enough history. Both "zero samples"
    // (mean is None) and "fewer than min_samples" land here — the
    // caller's `samples_found` is already 0 in the first case, so a
    // single branch covers both.
    let Some(mean) = mean.filter(|_| samples_found >= min_samples) else {
        return CheckOutcome::NotEnoughHistory {
            samples_found,
            required: min_samples,
        };
    };

    let red_ceiling = mean * (1.0 + thresholds.red);
    let yellow_ceiling = mean * (1.0 + thresholds.yellow);

    if current > red_ceiling {
        CheckOutcome::Red {
            current,
            mean,
            ceiling: red_ceiling,
        }
    } else if current > yellow_ceiling {
        CheckOutcome::Yellow {
            current,
            mean,
            ceiling: yellow_ceiling,
        }
    } else {
        CheckOutcome::Ok { current, mean }
    }
}

impl CheckOutcome {
    /// `true` only for [`CheckOutcome::Red`]. Useful for short-circuit
    /// "any red?" tallies without `match`ing on every call site.
    pub fn is_red(&self) -> bool {
        matches!(self, CheckOutcome::Red { .. })
    }

    /// Emit a one-line log entry at the severity appropriate to this
    /// outcome. `label` identifies the metric being reported (typically
    /// `"<measurement>.<field>"`). Keeps the per-variant rendering in
    /// one place so callers don't grow a four-arm `match` every time
    /// they need to surface a check result.
    pub fn log(&self, label: &str) {
        match self {
            CheckOutcome::Ok { current, mean } => {
                log::info!("  ok    {}: current={} mean={}", label, current, mean);
            }
            CheckOutcome::Yellow {
                current,
                mean,
                ceiling,
            } => {
                log::warn!(
                    "  YELLOW {}: current={} > yellow_ceiling={} (mean={})",
                    label,
                    current,
                    ceiling,
                    mean
                );
            }
            CheckOutcome::Red {
                current,
                mean,
                ceiling,
            } => {
                log::error!(
                    "  RED   {}: current={} > red_ceiling={} (mean={})",
                    label,
                    current,
                    ceiling,
                    mean
                );
            }
            CheckOutcome::NotEnoughHistory {
                samples_found,
                required,
            } => {
                log::info!(
                    "  skip  {}: {} historical samples (need {})",
                    label,
                    samples_found,
                    required
                );
            }
        }
    }
}

/// Hit InfluxDB to fetch historical mean, then evaluate. Convenience
/// wrapper for the CLI; tests should use [`evaluate`] directly.
pub async fn check(
    cfg: &InfluxConfig,
    branches: &[String],
    measurement: &str,
    field: &str,
    current: f64,
    min_samples: usize,
    thresholds: Thresholds,
) -> Result<CheckOutcome> {
    let hist = historical_mean(cfg, branches, measurement, field, min_samples).await?;
    Ok(evaluate(
        current,
        hist.as_ref().map(|h| h.mean),
        hist.as_ref().map(|h| h.samples_found).unwrap_or(0),
        min_samples,
        thresholds,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(yellow: f64, red: f64) -> Thresholds {
        Thresholds { yellow, red }
    }

    #[test]
    fn ok_when_under_yellow() {
        // mean=100, +10%=110, current=105 → OK
        assert_eq!(
            evaluate(105.0, Some(100.0), 10, 10, t(0.10, 0.20)),
            CheckOutcome::Ok {
                current: 105.0,
                mean: 100.0
            }
        );
    }

    #[test]
    fn yellow_when_between_yellow_and_red() {
        // mean=100, yellow ceiling=110, red ceiling=120, current=115
        let out = evaluate(115.0, Some(100.0), 10, 10, t(0.10, 0.20));
        match out {
            CheckOutcome::Yellow {
                current,
                mean,
                ceiling,
            } => {
                assert_eq!(current, 115.0);
                assert_eq!(mean, 100.0);
                assert!((ceiling - 110.0).abs() < 1e-9);
            }
            _ => panic!("expected Yellow, got {:?}", out),
        }
    }

    #[test]
    fn red_when_above_red_ceiling() {
        // mean=100, red ceiling=120, current=130 → Red
        let out = evaluate(130.0, Some(100.0), 10, 10, t(0.10, 0.20));
        match out {
            CheckOutcome::Red {
                current,
                mean,
                ceiling,
            } => {
                assert_eq!(current, 130.0);
                assert_eq!(mean, 100.0);
                assert!((ceiling - 120.0).abs() < 1e-9);
            }
            _ => panic!("expected Red, got {:?}", out),
        }
    }

    #[test]
    fn not_enough_history_when_no_samples() {
        let out = evaluate(100.0, None, 0, 10, Thresholds::default());
        assert_eq!(
            out,
            CheckOutcome::NotEnoughHistory {
                samples_found: 0,
                required: 10
            }
        );
    }

    #[test]
    fn not_enough_history_when_count_below_minimum() {
        let out = evaluate(100.0, Some(50.0), 3, 10, Thresholds::default());
        assert_eq!(
            out,
            CheckOutcome::NotEnoughHistory {
                samples_found: 3,
                required: 10
            }
        );
    }

    #[test]
    fn below_mean_is_always_ok() {
        // The regression check is one-sided: faster than the mean is
        // never bad.
        let out = evaluate(50.0, Some(100.0), 10, 10, t(0.10, 0.20));
        assert_eq!(
            out,
            CheckOutcome::Ok {
                current: 50.0,
                mean: 100.0
            }
        );
    }

    #[test]
    fn table_without_overrides_returns_the_default() {
        let table = ThresholdTable::new(t(0.10, 0.20));
        assert_eq!(table.resolve("SSPSS", "value").unwrap().red, 0.20);
    }

    #[test]
    fn bare_field_override_covers_every_measurement() {
        // The case this exists for: one key, all 17 snark permutations.
        let table =
            ThresholdTable::new(t(0.10, 0.20)).with_override("verification time", t(0.40, 0.60));
        for shape in ["SSPSS", "SPPPP", "SSS"] {
            assert_eq!(table.resolve(shape, "verification time").unwrap().red, 0.60);
            // and the quiet field on the same record keeps the tight gate
            assert_eq!(table.resolve(shape, "value").unwrap().red, 0.20);
        }
    }

    #[test]
    fn qualified_key_beats_bare_field() {
        let table = ThresholdTable::new(t(0.10, 0.20))
            .with_override("verification time", t(0.40, 0.60))
            .with_override("SSS.verification time", t(0.05, 0.10));
        assert_eq!(table.resolve("SSS", "verification time").unwrap().red, 0.10);
        assert_eq!(table.resolve("SPP", "verification time").unwrap().red, 0.60);
    }

    #[test]
    fn measurement_containing_a_dot_resolves_whole() {
        // Archive measurements are operation names with dots in them, so
        // the key must never be split on '.'.
        let table = ThresholdTable::new(t(0.10, 0.20))
            .with_override("Zkapp_account_update.add.avg_time_ms", t(0.5, 0.9));
        assert_eq!(
            table
                .resolve("Zkapp_account_update.add", "avg_time_ms")
                .unwrap()
                .red,
            0.9
        );
        // A different operation with the same field keeps the default.
        assert_eq!(table.resolve("Block.add", "avg_time_ms").unwrap().red, 0.20);
    }

    #[test]
    fn excluded_metric_resolves_to_none() {
        let table = ThresholdTable::new(t(0.10, 0.20)).with_exclusion("verification time");
        assert!(table.resolve("SSPSS", "verification time").is_none());
        assert!(table.resolve("SSPSS", "value").is_some());
    }

    #[test]
    fn exclusion_wins_over_an_override() {
        let table = ThresholdTable::new(t(0.10, 0.20))
            .with_override("verification time", t(0.4, 0.6))
            .with_exclusion("verification time");
        assert!(table.resolve("SSPSS", "verification time").is_none());
    }

    #[test]
    fn field_threshold_spec_parses() {
        let (key, th) = parse_field_threshold("verification time=0.4,0.6").unwrap();
        assert_eq!(key, "verification time");
        assert_eq!(th.yellow, 0.4);
        assert_eq!(th.red, 0.6);
    }

    #[test]
    fn field_threshold_spec_splits_on_the_last_equals() {
        let (key, th) = parse_field_threshold("odd=name=0.1,0.2").unwrap();
        assert_eq!(key, "odd=name");
        assert_eq!(th.red, 0.2);
    }

    #[test]
    fn field_threshold_spec_rejects_malformed_input() {
        for bad in [
            "verification time",          // no '='
            "verification time=0.4",      // no ','
            "=0.4,0.6",                   // empty key
            "verification time=abc,0.6",  // not a number
            "verification time=-0.1,0.6", // negative
            "verification time=0.6,0.4",  // yellow above red
        ] {
            assert!(
                parse_field_threshold(bad).is_err(),
                "expected {:?} to be rejected",
                bad
            );
        }
    }

    #[test]
    fn boundary_value_exactly_at_yellow_ceiling_is_ok() {
        // `value > ceiling` is strict — equal-to-ceiling counts as OK.
        // Documenting the choice; reasonable people could disagree.
        let out = evaluate(110.0, Some(100.0), 10, 10, t(0.10, 0.20));
        assert_eq!(
            out,
            CheckOutcome::Ok {
                current: 110.0,
                mean: 100.0
            }
        );
    }
}
