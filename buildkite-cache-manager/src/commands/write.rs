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

/// Execute the write command: copy local files to cache.
pub fn execute(
    backend: &dyn CacheBackend,
    cache_base: &str,
    root: &str,
    input: &str,
    output: &str,
    override_existing: bool,
    format: &OutputFormat,
) -> Result<()> {
    let cache_path = PathBuf::from(format!("{}/{}/{}", cache_base, root, output));

    backend
        .create_dir_all(&cache_path)
        .context("Failed to create cache directories")?;

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

    match format {
        OutputFormat::Text => println!("Done."),
        OutputFormat::Json => {
            let result = WriteResult {
                source: input.to_string(),
                destination: cache_path.to_string_lossy().to_string(),
                status: "ok",
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}
