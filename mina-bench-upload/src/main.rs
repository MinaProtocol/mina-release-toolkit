use anyhow::{anyhow, Context, Result};
use clap::{Parser as ClapParser, ValueEnum};
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use mina_bench_upload::config::InfluxConfig;
use mina_bench_upload::influx;
use mina_bench_upload::parse::{
    self, archive::ArchiveParser, generic::GenericJsonParser, heap::HeapParser,
    janestreet::JaneStreetParser, ledger_apply::LedgerApplyParser, snark::SnarkParser,
    zkapp::ZkappParser, Parser,
};
use mina_bench_upload::regression::{self, ThresholdTable, Thresholds};

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
    /// Generic passthrough: a JSON array of {measurement, tags, fields,
    /// timestamp_ns} records. For projects that emit their own metrics.
    GenericJson,
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
    /// `gitbranch` tag on every record, and the default regression
    /// baseline when no `--compare-branch` is given.
    #[arg(long)]
    branch: String,

    /// Branch(es) whose history forms the regression baseline. Repeatable
    /// (e.g. `--compare-branch develop --compare-branch master`); the
    /// check compares against the union of their samples. A wider, more
    /// stable baseline than a single branch, which matters for noisy
    /// timing benches. Defaults to `--branch` when omitted.
    #[arg(long = "compare-branch")]
    compare_branch: Vec<String>,

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

    /// Override `--yellow`/`--red` for one metric. Repeatable.
    ///
    /// `<key>=<yellow>,<red>`, where the key is either a field name
    /// (`'verification time=0.4,0.6'`, applying to that field on every
    /// measurement) or a fully qualified `<measurement>.<field>`
    /// (`'SSS.verification time=0.4,0.6'`), which takes precedence.
    /// Use it when fields of the same benchmark have different
    /// run-to-run variance and so cannot share one threshold.
    #[arg(long = "field-threshold", value_name = "KEY=YELLOW,RED")]
    field_thresholds: Vec<String>,

    /// Exclude a metric from the regression gate. Repeatable, same key
    /// form as `--field-threshold`. The metric is still parsed and
    /// uploaded, so its trend stays visible; it just never fails the
    /// build.
    #[arg(long = "exclude-field", value_name = "KEY")]
    exclude_fields: Vec<String>,

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
    let records = match read_and_parse(&cli) {
        Ok(r) => r,
        Err(code) => return code,
    };

    let cfg = match load_influx_config(&cli) {
        Ok(c) => c,
        Err(code) => return code,
    };

    let thresholds = match build_threshold_table(&cli) {
        Ok(t) => t,
        Err(code) => return code,
    };

    let saw_red = if cli.check_regression {
        run_regression_phase(&cli, cfg.as_ref(), &records, &thresholds).await
    } else {
        false
    };

    if cli.upload {
        if let Err(code) = run_upload_phase(&cli, cfg.as_ref(), &records).await {
            return code;
        }
    }

    if saw_red {
        EXIT_RED_REGRESSION
    } else {
        EXIT_OK
    }
}

/// Read stdin/file and parse into records. On any failure, log a
/// diagnostic and return the matching exit code so the caller can
/// short-circuit.
fn read_and_parse(cli: &Cli) -> Result<Vec<parse::BenchmarkRecord>, u8> {
    let raw = read_input(&cli.input).map_err(|e| {
        log::error!("Failed to read input: {:#}", e);
        EXIT_PARSE_ERROR
    })?;
    let records = parse_input(cli.format, &raw, &cli.branch).map_err(|e| {
        log::error!("Parse error: {:#}", e);
        EXIT_PARSE_ERROR
    })?;
    log::info!("Parsed {} record(s)", records.len());
    for r in &records {
        log::debug!(
            "record: measurement={} tags={:?} fields={:?}",
            r.measurement,
            r.tags,
            r.fields
        );
    }
    Ok(records)
}

/// Build the metric → thresholds table from the global `--yellow`/`--red`
/// plus any `--field-threshold` / `--exclude-field` overrides. A
/// malformed override is a configuration error: silently ignoring it
/// would leave the build gated at a threshold the caller did not ask for.
fn build_threshold_table(cli: &Cli) -> Result<ThresholdTable, u8> {
    let mut table = ThresholdTable::new(Thresholds {
        yellow: cli.yellow,
        red: cli.red,
    });
    for spec in &cli.field_thresholds {
        let (key, thresholds) = regression::parse_field_threshold(spec).map_err(|e| {
            log::error!("Invalid --field-threshold: {:#}", e);
            EXIT_CONFIG_ERROR
        })?;
        log::info!(
            "threshold override for {:?}: yellow=+{:.0}%, red=+{:.0}%",
            key,
            thresholds.yellow * 100.0,
            thresholds.red * 100.0
        );
        table = table.with_override(key, thresholds);
    }
    for key in &cli.exclude_fields {
        let key = key.trim();
        if key.is_empty() {
            log::error!("Invalid --exclude-field: empty metric key");
            return Err(EXIT_CONFIG_ERROR);
        }
        log::info!("{:?} excluded from the regression gate", key);
        table = table.with_exclusion(key);
    }
    Ok(table)
}

/// Load InfluxDB env-var config when the chosen flags require it.
/// Returns `Ok(None)` when no upload / regression check is requested
/// (or when `--dry-run` short-circuits the network); returns an exit
/// code on validation failure.
fn load_influx_config(cli: &Cli) -> Result<Option<InfluxConfig>, u8> {
    let needs_influx = (cli.upload || cli.check_regression) && !cli.dry_run;
    if !needs_influx {
        return Ok(None);
    }
    InfluxConfig::from_env().map(Some).map_err(|e| {
        log::error!("InfluxDB configuration error: {:#}", e);
        EXIT_CONFIG_ERROR
    })
}

/// Run the regression check (or log a dry-run summary). Returns
/// whether any field tripped the red threshold.
async fn run_regression_phase(
    cli: &Cli,
    cfg: Option<&InfluxConfig>,
    records: &[parse::BenchmarkRecord],
    thresholds: &ThresholdTable,
) -> bool {
    if cli.dry_run {
        log::info!(
            "[dry-run] would check regression: {} record(s) against last {} samples (default yellow=+{:.0}%, red=+{:.0}%)",
            records.len(),
            cli.min_samples,
            cli.yellow * 100.0,
            cli.red * 100.0
        );
        return false;
    }
    let Some(cfg) = cfg else {
        return false;
    };
    // Regression baseline: the explicit `--compare-branch` set, or
    // `--branch` when none was given.
    let baseline: Vec<String> = if cli.compare_branch.is_empty() {
        vec![cli.branch.clone()]
    } else {
        cli.compare_branch.clone()
    };
    run_regression_checks(cfg, records, &baseline, cli.min_samples, thresholds).await
}

/// Push records to InfluxDB (or log a dry-run summary).
async fn run_upload_phase(
    cli: &Cli,
    cfg: Option<&InfluxConfig>,
    records: &[parse::BenchmarkRecord],
) -> Result<(), u8> {
    if cli.dry_run {
        log::info!(
            "[dry-run] would upload {} record(s) to InfluxDB",
            records.len()
        );
        return Ok(());
    }
    let Some(cfg) = cfg else {
        return Ok(());
    };
    match influx::upload(records, cfg).await {
        Ok(n) => {
            log::info!("Uploaded {} record(s) to {}/{}", n, cfg.host, cfg.bucket);
            Ok(())
        }
        Err(e) => {
            log::error!("Upload failed: {:#}", e);
            Err(EXIT_UPLOAD_ERROR)
        }
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
        Format::GenericJson => GenericJsonParser.parse(input, branch)?,
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
    baseline: &[String],
    min_samples: usize,
    thresholds: &ThresholdTable,
) -> bool {
    let mut saw_red = false;
    for record in records {
        for (field_name, field_value) in &record.fields {
            let label = format!("{}.{}", record.measurement, field_name);
            let Some(field_thresholds) = thresholds.resolve(&record.measurement, field_name) else {
                log::info!("  skip  {}: excluded from the regression gate", label);
                continue;
            };
            match regression::check(
                cfg,
                baseline,
                &record.measurement,
                field_name,
                field_value.as_f64(),
                min_samples,
                field_thresholds,
            )
            .await
            {
                Ok(outcome) => {
                    outcome.log(&label);
                    saw_red |= outcome.is_red();
                }
                Err(e) => {
                    log::warn!(
                        "regression query for {} failed: {:#} — skipping check, build will not be failed by this",
                        label,
                        e
                    );
                }
            }
        }
    }
    saw_red
}
