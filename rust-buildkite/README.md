# Buildkite Pipeline Schema Toolkit

This crate vendors the official Buildkite pipeline schema and exposes a minimal
DSL for constructing pipelines. You can either work directly with
`buildkite_pipeline::schema::*` or use the light-weight builders to produce
YAML/JSON.

## Getting Started

```bash
cargo add buildkite-pipeline
```

```rust
use buildkite_pipeline::{CommandStepBuilder, PipelineBuilder};

fn main() {
    let pipeline = PipelineBuilder::new()
        .env("CI", "true")
        .step(CommandStepBuilder::new("cargo test").label(":test_tube: Tests"))
        .build();

    println!("{}", pipeline.to_yaml().unwrap());
}
```

## Development

- Format/lint/tests: `cargo fmt`, `cargo clippy`, `cargo test`
- Core modules live in `src/pipeline.rs`, `src/steps.rs`, and `src/schema.rs`

### Schema Tracking & Generation

`schema/pipeline.schema.json` mirrors the upstream Buildkite schema referenced
in `src/version.rs`. The `build.rs` script runs Typify to generate
`buildkite_pipeline::schema::*` on every build/test. Helpers include:

- `cargo run --bin schema_tool -- show`
- `cargo run --bin schema_tool -- download`
- `cargo run --bin schema_tool -- compare`

Workflow for bumping the schema:

1. Download the new JSON into `schema/pipeline.schema.json`.
2. Update `src/version.rs` with the commit/date/url.
3. Run `cargo test` to regenerate bindings.
4. Extend the high-level builders if needed.

## License

This project is released under the MIT License (see `LICENSE`).
