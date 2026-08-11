# mina-bench-upload

Parse Mina benchmark output and upload the results to InfluxDB.

This crate is the successor to the Python tool at
[`mina/scripts/benchmarks/`](https://github.com/MinaProtocol/mina/tree/develop/scripts/benchmarks),
but with one deliberate architectural change:

> **The new tool no longer executes the benchmark binary.**
> The dhall/bash CI layer is responsible for running the benchmark
> and piping its stdout to `mina-bench-upload` on stdin (or via
> `--input <file>`). This tool only parses, checks for regressions,
> and uploads.

## Why the change

The old Python tool conflated three responsibilities — execution,
parsing, and upload — into a single 768-line module with one big
abstract base class. Spawning subprocesses from the same tool that
ships results made it hard to test (every test needed the binary),
hard to diagnose (a benchmark crash and an upload failure looked
similar), and impossible to reuse (you couldn't upload an old captured
output without re-running it).

Splitting the responsibilities means:
* CI can run benchmarks with whatever runner (dune, mina, plain bash)
  is appropriate and pipe output here.
* You can re-upload a saved benchmark output by feeding the file in.
* Tests run without any benchmark binary present.

## Usage

```bash
# In CI / from dhall+bash:
mina-benchmarks <args> | mina-bench-upload \
  --format mina-base \
  --branch "$BUILDKITE_BRANCH" \
  --upload \
  --check-regression

# From a saved capture:
mina-bench-upload \
  --format mina-base \
  --input ./saved-output.txt \
  --branch develop \
  --upload \
  --check-regression
```

### Supported formats

| `--format`        | Source benchmark           | Notes                                                        |
| ----------------- | -------------------------- | ------------------------------------------------------------ |
| `mina-base`       | `mina-benchmarks`          | JaneStreet `core_bench` markdown table (`│`-delimited).      |
| `ledger-export`   | `mina-ledger-export-benchmark` | Same shape as `mina-base`, just a different `category` tag. |
| `snark`           | `mina transaction-snark-profiler` | Pipe-delimited markdown table.                         |
| `zkapp`           | `mina-zkapp-limits`        | Free-text `Proofs updates=N…Cost: X.X` lines.                |
| `archive`         | archive-node bench         | JSON array of `{operation, avg_time_ms}`.                    |
| `heap`            | `mina-heap-usage`          | `Data of type X uses Y heap words = Z bytes`.                |
| `ledger-apply`    | ledger apply test          | JSON object with `final_time` + `preparation_steps_mean`.    |
| `generic-json`    | any project                | JSON array of `{measurement, tags, fields, timestamp_ns}` — a passthrough for benches that emit their own metrics (no bespoke parser needed). |

The InfluxDB measurement / tag / field names this tool writes match
what the Python tool was writing historically, so the regression
check still resolves against the existing samples.

### Flags

| Flag                     | Default | Meaning                                                                                        |
| ------------------------ | ------- | ---------------------------------------------------------------------------------------------- |
| `--format <fmt>`         | —       | Required. One of the formats above.                                                            |
| `--input <path>`         | `-`     | File path, or `-` for stdin.                                                                   |
| `--branch <name>`        | —       | Required. Written to the `gitbranch` tag.                                                      |
| `--upload`               | off     | Actually push to InfluxDB. Without it, the tool only parses and logs.                          |
| `--check-regression`     | off     | Run the historical-mean regression check.                                                      |
| `--yellow <fraction>`    | `0.10`  | Fraction over mean above which we warn.                                                        |
| `--red <fraction>`       | `0.20`  | Fraction over mean above which we fail the build.                                              |
| `--field-threshold <k=y,r>` | —    | Override `--yellow`/`--red` for one metric. Repeatable. See below.                             |
| `--exclude-field <key>`  | —       | Never gate the build on this metric. Repeatable. It is still parsed and uploaded.              |
| `--min-samples <n>`      | `10`    | Minimum historical samples required before the check runs.                                     |
| `--dry-run`              | off     | Parse + log what would be sent, but don't hit InfluxDB.                                        |

### Exit codes

| Code | Meaning                                                |
| ---- | ------------------------------------------------------ |
| 0    | OK (no red regression, or `--check-regression` not set)|
| 1    | Red regression detected                                |
| 2    | Parse error (wrong `--format`, malformed input)        |
| 3    | Upload error (network, auth, InfluxDB rejection)       |
| 4    | Configuration error (missing required env var)         |

### Environment

When `--upload` or `--check-regression` is set, the tool reads:

* `INFLUX_HOST` — host or full URL. `https://` is prepended if missing.
* `INFLUX_TOKEN` — bearer token.
* `INFLUX_ORG` — InfluxDB organization name or id.
* `INFLUX_BUCKET_NAME` — target bucket.

These match the variables the Python tool reads, so existing CI secrets
work as-is. Missing variables produce a single useful error and exit 4
*before* any parsing happens (the Python tool validated late, at
upload time, which masked misconfiguration until after a long run).

## Regression-check semantics

For each `(branch, measurement, field)` tuple, fetch the most recent
`--min-samples` historical values, compute their arithmetic mean, and
classify the current value:

* `current ≤ mean * (1 + yellow)` → **OK**
* `mean * (1 + yellow) < current ≤ mean * (1 + red)` → **Yellow**, warn
* `current > mean * (1 + red)` → **Red**, exit 1
* fewer than `--min-samples` historical points → **skip**, log info

The Python tool had a long-standing bug at `bench.py:137` —
`isclose(value + red_threshold, average)` — that silently masked
regressions. The Rust check uses the correct `value > mean * (1 + threshold)`
comparison.

### Per-metric thresholds

One threshold cannot fit a benchmark whose fields differ in run-to-run
variance. The snark bench produces both kinds on every run. Measured over
four consecutive `develop` nightlies, 17 permutations each:

| field               | run-to-run spread of the same permutation | red verdicts at `red=0.20` |
| ------------------- | ----------------------------------------- | -------------------------- |
| `value`             | median 1.0%, max 1.9%                     | 0 / 68                     |
| `verification time` | median 30.4%, max 87%                     | 16 / 68 (24%)              |

None of those 16 was a regression — the median `current / mean` was 1.06,
i.e. sitting on the historical mean. But with 17 permutations checked
independently, a 24% per-sample trip rate is a **99% chance of failing the
build every night** on noise alone. Raising the global `--red` to cover
`verification time` would drop `value` from a 20% gate to a 60% one and
lose the check that works.

So pick the threshold per metric:

```bash
mina-bench-upload --format snark --branch develop --check-regression \
  --field-threshold 'verification time=0.4,0.6'
```

A key is matched whole, first as `<measurement>.<field>` and then as
`<field>`:

* `'verification time=0.4,0.6'` — that field on **every** measurement.
  One key covers all 17 snark permutations.
* `'SSS.verification time=0.4,0.6'` — one permutation only. Takes
  precedence over the bare field name.

The key is never split on `.`, so measurements whose names contain dots
(`Zkapp_account_update.add`) work as written:
`'Zkapp_account_update.add.avg_time_ms=0.5,0.9'`.

To keep a metric visible in InfluxDB without ever failing the build:

```bash
mina-bench-upload ... --exclude-field 'verification time'
```

A malformed `--field-threshold` exits 4 rather than falling back to the
global value — otherwise a typo would silently gate the build at a
threshold nobody chose.

## Development

```bash
cargo build
cargo test           # 45 unit tests + 9 CLI smoke tests
cargo clippy --no-deps --all-targets -- -D warnings
cargo fmt --check
```

The integration tests under `tests/` use `--dry-run` and never hit a
real InfluxDB; they're safe to run anywhere.
