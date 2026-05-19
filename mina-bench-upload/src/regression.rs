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
use anyhow::Result;

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
    branch: &str,
    measurement: &str,
    field: &str,
    current: f64,
    min_samples: usize,
    thresholds: Thresholds,
) -> Result<CheckOutcome> {
    let hist = historical_mean(cfg, branch, measurement, field, min_samples).await?;
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
