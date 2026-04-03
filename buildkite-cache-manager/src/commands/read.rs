use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::cache::CacheBackend;
use crate::cli::OutputFormat;

#[derive(Serialize)]
struct ReadResult {
    source: String,
    destination: String,
    status: &'static str,
}

/// Execute the read command: copy files from cache to local destination.
pub fn execute(
    backend: &dyn CacheBackend,
    cache_base: &str,
    root: &str,
    input: &str,
    output: &str,
    override_existing: bool,
    skip_dirs_create: bool,
    format: &OutputFormat,
) -> Result<()> {
    let cache_path = format!("{}/{}/{}", cache_base, root, input);
    let local_path = PathBuf::from(output);

    if !skip_dirs_create {
        backend
            .create_dir_all(&local_path)
            .context("Failed to create local directories")?;
    }

    if !backend.is_dir(&local_path) {
        anyhow::bail!(
            "Local location does not exist (or permission denied): '{}'\n\
             HINT: allow to create local dirs by not using '--skip-dirs-create'",
            local_path.display()
        );
    }

    if *format == OutputFormat::Text {
        println!("..Copying {} -> {}", cache_path, local_path.display());
    }

    // If the path contains glob characters, use glob copy
    if cache_path.contains('*') || cache_path.contains('?') || cache_path.contains('[') {
        backend.copy_glob(&cache_path, &local_path, override_existing)?;
    } else {
        let src = PathBuf::from(&cache_path);
        backend.copy(&src, &local_path, override_existing)?;
    }

    match format {
        OutputFormat::Text => println!("Done."),
        OutputFormat::Json => {
            let result = ReadResult {
                source: cache_path,
                destination: local_path.to_string_lossy().to_string(),
                status: "ok",
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}
