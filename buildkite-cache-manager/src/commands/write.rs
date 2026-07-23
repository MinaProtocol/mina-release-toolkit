use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::cache::CacheBackend;
use crate::cli::OutputFormat;

#[derive(Serialize)]
struct WriteResult {
    source: String,
    destination: String,
    status: &'static str,
}

/// Execute the write command: copy one or more local inputs into a cache
/// directory. Mirrors the bash `write-to-dir`: every input (literal path or
/// glob) is copied into `output`, which is created as a directory.
pub fn execute(
    backend: &dyn CacheBackend,
    cache_base: &str,
    root: &str,
    inputs: &[String],
    output: &str,
    override_existing: bool,
    format: &OutputFormat,
) -> Result<()> {
    let cache_path = PathBuf::from(format!("{}/{}/{}", cache_base, root, output));

    backend
        .create_dir_all(&cache_path)
        .context("Failed to create cache directories")?;

    let mut results = Vec::with_capacity(inputs.len());
    for input in inputs {
        if *format == OutputFormat::Text {
            println!("..Copying {} -> {}", input, cache_path.display());
        }

        // If the input contains glob characters, use glob copy
        if input.contains('*') || input.contains('?') || input.contains('[') {
            backend.copy_glob(input, &cache_path, override_existing)?;
        } else {
            let src = PathBuf::from(input);
            backend.copy(&src, &cache_path, override_existing)?;
        }

        results.push(WriteResult {
            source: input.clone(),
            destination: cache_path.to_string_lossy().to_string(),
            status: "ok",
        });
    }

    match format {
        OutputFormat::Text => println!("Done."),
        // An array, one entry per input, so single- and multi-input writes have
        // the same shape.
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&results)?),
    }

    Ok(())
}
