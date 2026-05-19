use anyhow::{anyhow, Context, Result};
use clap::{Parser as ClapParser, ValueEnum};
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use mina_bench_upload::config::InfluxConfig;
use mina_bench_upload::influx;
use mina_bench_upload::parse::{
    self, archive::ArchiveParser, heap::HeapParser, janestreet::JaneStreetParser,
    ledger_apply::LedgerApplyParser, snark::SnarkParser, zkapp::ZkappParser, Parser,
};
use mina_bench_upload::regression::{self, CheckOutcome, Thresholds};

/// Exit codes — stable contract for callers in dhall/bash.
const EXIT_OK: u8 = 0;
const EXIT_RED_REGRESSION: u8 = 1;
const EXIT_PARSE_ERROR: u8 = 2;
const EXIT_UPLOAD_ERROR: u8 = 3;
const EXIT_CONFIG_ERROR: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum Format {
    MinaBase,
    LedgerExport,
    Snark,
    Zkapp,
    Archive,
    Heap,
    LedgerApply,
}

#[derive(Debug, ClapParser)]
#[command(
    name = "mina-bench-upload",
    about = "Parse Mina benchmark output and upload to InfluxDB. Does NOT execute the benchmark binary — pipe its stdout in or pass --input."
)]
struct Cli {
    /// Output format produced by the benchmark binary.
    #[arg(long, value_enum)]
    format: Format,

    /// Path to a file containing the benchmark stdout, or `-` to read
    /// from stdin (default).
    #[arg(long, default_value = "-")]
    input: String,

    /// The git branch the benchmark was run from. Recorded as the
    /// `gitbranch` tag on every record. The same value is used for
    /// the regression query filter.
    #[arg(long)]
    branch: String,

    /// Upload parsed records to InfluxDB. Without it, the tool only
    /// prints what it parsed (and runs the regression check if
    /// `--check-regression` is set against existing data).
    #[arg(long)]
    upload: bool,

    /// Run the historical-mean regression check. Exits non-zero
    /// (`EXIT_RED_REGRESSION`) if any field on any record exceeds
    /// `mean * (1 + red)`.
    #[arg(long)]
    check_regression: bool,

    /// Fraction over the historical mean above which the build emits
    /// a warning. e.g. `0.10` = +10%.
    #[arg(long, default_value_t = 0.10)]
    yellow: f64,

    /// Fraction over the historical mean above which the build fails.
    /// e.g. `0.20` = +20%.
    #[arg(long, default_value_t = 0.20)]
    red: f64,

    /// Minimum number of historical samples required before the
    /// regression check is meaningful. Below this, the check skips
    /// with a warning.
    #[arg(long, default_value_t = 10)]
    min_samples: usize,

    /// Parse + log what would be sent, but don't actually call
    /// InfluxDB. Implies `--upload` is observed for accounting only.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();
    let cli = Cli::parse();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let exit = rt.block_on(run(cli));
    ExitCode::from(exit)
}

async fn run(cli: Cli) -> u8 {
    // ---- read input ----
    let raw = match read_input(&cli.input) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to read input: {:#}", e);
            return EXIT_PARSE_ERROR;
        }
    };

    // ---- parse ----
    let records = match parse_input(cli.format, &raw, &cli.branch) {
        Ok(r) => r,
        Err(e) => {
            log::error!("Parse error: {:#}", e);
            return EXIT_PARSE_ERROR;
        }
    };
    log::info!("Parsed {} record(s)", records.len());
    for r in &records {
        log::debug!(
            "record: measurement={} tags={:?} fields={:?}",
            r.measurement,
            r.tags,
            r.fields
        );
    }

    // ---- env-var config (only needed if we'll talk to InfluxDB) ----
    let needs_influx = cli.upload || cli.check_regression;
    let cfg = if needs_influx && !cli.dry_run {
        match InfluxConfig::from_env() {
            Ok(c) => Some(c),
            Err(e) => {
                log::error!("InfluxDB configuration error: {:#}", e);
                return EXIT_CONFIG_ERROR;
            }
        }
    } else {
        None
    };

    // ---- regression check ----
    let thresholds = Thresholds {
        yellow: cli.yellow,
        red: cli.red,
    };
    let mut saw_red = false;
    if cli.check_regression {
        if cli.dry_run {
            log::info!(
                "[dry-run] would check regression: {} record(s) against last {} samples (yellow=+{:.0}%, red=+{:.0}%)",
                records.len(),
                cli.min_samples,
                thresholds.yellow * 100.0,
                thresholds.red * 100.0
            );
        } else if let Some(cfg) = &cfg {
            saw_red = run_regression_checks(cfg, &records, cli.min_samples, thresholds).await;
        }
    }

    // ---- upload ----
    if cli.upload {
        if cli.dry_run {
            log::info!(
                "[dry-run] would upload {} record(s) to InfluxDB",
                records.len()
            );
        } else if let Some(cfg) = &cfg {
            match influx::upload(&records, cfg).await {
                Ok(n) => log::info!("Uploaded {} record(s) to {}/{}", n, cfg.host, cfg.bucket),
                Err(e) => {
                    log::error!("Upload failed: {:#}", e);
                    return EXIT_UPLOAD_ERROR;
                }
            }
        }
    }

    if saw_red {
        EXIT_RED_REGRESSION
    } else {
        EXIT_OK
    }
}

fn read_input(path: &str) -> Result<String> {
    if path == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("reading from stdin")?;
        Ok(s)
    } else {
        std::fs::read_to_string(PathBuf::from(path)).with_context(|| format!("reading {}", path))
    }
}

fn parse_input(format: Format, input: &str, branch: &str) -> Result<Vec<parse::BenchmarkRecord>> {
    let records = match format {
        Format::MinaBase => JaneStreetParser::mina_base().parse(input, branch)?,
        Format::LedgerExport => JaneStreetParser::ledger_export().parse(input, branch)?,
        Format::Snark => SnarkParser.parse(input, branch)?,
        Format::Zkapp => ZkappParser.parse(input, branch)?,
        Format::Archive => ArchiveParser.parse(input, branch)?,
        Format::Heap => HeapParser.parse(input, branch)?,
        Format::LedgerApply => LedgerApplyParser.parse(input, branch)?,
    };
    if records.is_empty() {
        return Err(anyhow!(
            "no records parsed from input — wrong --format or empty input?"
        ));
    }
    Ok(records)
}

/// Run a regression check for every (record, field) pair and log the
/// outcome. Returns `true` if any field tripped the red threshold.
async fn run_regression_checks(
    cfg: &InfluxConfig,
    records: &[parse::BenchmarkRecord],
    min_samples: usize,
    thresholds: Thresholds,
) -> bool {
    let mut saw_red = false;
    for record in records {
        let Some(branch) = record.tags.get(parse::TAG_GITBRANCH) else {
            log::warn!(
                "record {} has no gitbranch tag; skipping regression",
                record.measurement
            );
            continue;
        };
        for (field_name, field_value) in &record.fields {
            let outcome = match regression::check(
                cfg,
                branch,
                &record.measurement,
                field_name,
                field_value.as_f64(),
                min_samples,
                thresholds,
            )
            .await
            {
                Ok(o) => o,
                Err(e) => {
                    log::warn!(
                        "regression query for {}.{} failed: {:#} — skipping check, build will not be failed by this",
                        record.measurement,
                        field_name,
                        e
                    );
                    continue;
                }
            };
            match outcome {
                CheckOutcome::Ok { current, mean } => {
                    log::info!(
                        "  ok    {}.{}: current={} mean={}",
                        record.measurement,
                        field_name,
                        current,
                        mean
                    );
                }
                CheckOutcome::Yellow {
                    current,
                    mean,
                    ceiling,
                } => {
                    log::warn!(
                        "  YELLOW {}.{}: current={} > yellow_ceiling={} (mean={})",
                        record.measurement,
                        field_name,
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
                        "  RED   {}.{}: current={} > red_ceiling={} (mean={})",
                        record.measurement,
                        field_name,
                        current,
                        ceiling,
                        mean
                    );
                    saw_red = true;
                }
                CheckOutcome::NotEnoughHistory {
                    samples_found,
                    required,
                } => {
                    log::info!(
                        "  skip  {}.{}: {} historical samples (need {})",
                        record.measurement,
                        field_name,
                        samples_found,
                        required
                    );
                }
            }
        }
    }
    saw_red
}
