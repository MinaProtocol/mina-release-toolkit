use colored::*;
use std::path::Path;

use crate::cli::ReversionArgs;
use crate::errors::{ManagerError, ManagerResult};
use crate::reversion::reversion_debian_package;

/// Walk `{source_folder}/{codename}/*.deb` and reversion every package into
/// `{output_folder}/{codename}/`. Mirrors `manager.sh reversion`.
pub async fn execute(args: ReversionArgs) -> ManagerResult<()> {
    let source = Path::new(&args.source_folder);
    if !source.is_dir() {
        return Err(ManagerError::ValidationError(format!(
            "Source folder does not exist: {}",
            args.source_folder
        )));
    }

    println!();
    println!(" ℹ️  Reversioning .deb packages with following parameters:");
    println!(" - Source folder: {}", args.source_folder);
    println!(" - Output folder: {}", args.output_folder);
    println!(" - New version: {}", args.new_version);
    if let Some(s) = &args.suite {
        println!(" - Suite: {}", s);
    }
    if let Some(n) = &args.name {
        println!(" - Rename to: {}", n);
    }

    tokio::fs::create_dir_all(&args.output_folder).await?;

    let mut total = 0usize;
    let mut success = 0usize;
    let mut fail = 0usize;

    let mut codename_dirs = tokio::fs::read_dir(source).await?;
    while let Some(entry) = codename_dirs.next_entry().await? {
        let codename_path = entry.path();
        if !codename_path.is_dir() {
            continue;
        }
        let codename = codename_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let output_codename_dir = Path::new(&args.output_folder).join(&codename);
        tokio::fs::create_dir_all(&output_codename_dir).await?;

        let mut debs = tokio::fs::read_dir(&codename_path).await?;
        while let Some(deb_entry) = debs.next_entry().await? {
            let deb_path = deb_entry.path();
            let basename = match deb_path.file_name().and_then(|s| s.to_str()) {
                Some(name) if name.ends_with(".deb") => name.to_string(),
                _ => continue,
            };

            total += 1;
            println!(
                "  🔄  Reversioning {}/{} -> version {}",
                codename, basename, args.new_version
            );

            // Filename pattern: {name}_{version}_{arch}.deb
            let stem = basename.trim_end_matches(".deb");
            let parts: Vec<&str> = stem.rsplitn(3, '_').collect(); // [arch, version, name]
            if parts.len() != 3 {
                println!(
                    "  ⚠️  Warning: cannot parse name/version/arch from {} — skipping",
                    basename
                );
                fail += 1;
                continue;
            }
            let pkg_arch = parts[0];
            let pkg_version = parts[1];
            let pkg_name = parts[2];
            let final_name = args.name.as_deref().unwrap_or(pkg_name);
            let output_file = output_codename_dir
                .join(format!("{}_{}_{}.deb", final_name, args.new_version, pkg_arch));

            // The internal reversion_debian_package writes alongside the source;
            // we then move into the requested output path.
            let suite = args.suite.as_deref().unwrap_or("unstable");
            let result = reversion_debian_package(
                &deb_path,
                pkg_name,
                pkg_version,
                &args.new_version,
                suite,
                suite,
                Some(final_name),
            )
            .await;

            match result {
                Ok(produced) => {
                    if produced != output_file {
                        if let Err(e) = tokio::fs::rename(&produced, &output_file).await {
                            println!(
                                "  ⚠️  Reversion succeeded but rename to {} failed: {}",
                                output_file.display(),
                                e
                            );
                            fail += 1;
                            continue;
                        }
                    }
                    success += 1;
                }
                Err(e) => {
                    println!(
                        "  ⚠️  Warning: failed to reversion {}/{}: {} — skipping",
                        codename, basename, e
                    );
                    fail += 1;
                }
            }
        }
    }

    println!();
    if total == 0 {
        println!(
            " ⚠️  No .deb files found in {}/{{codename}}/ subdirectories.",
            args.source_folder
        );
    } else {
        println!(
            " ℹ️  Summary: {}/{} packages reversioned successfully.",
            success, total
        );
        if fail > 0 {
            println!(
                "{}",
                format!(" ⚠️  {} package(s) failed to reversion.", fail).yellow()
            );
        }
    }
    println!(" ✅  Done.");
    println!();
    Ok(())
}
