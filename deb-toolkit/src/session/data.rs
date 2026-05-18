//! Mutations on the package data tree (the `data/` directory of a session).

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::Session;

/// Copy one or more `sources` into the package at `dest`.
///
/// When `as_directory` is true (or when more than one source is given),
/// `dest` is treated as a directory and each source keeps its filename.
/// Otherwise `dest` is taken to be the full destination path.
pub fn insert(
    session: &Session,
    dest: &str,
    sources: &[PathBuf],
    as_directory: bool,
) -> Result<()> {
    if sources.is_empty() {
        return Err(anyhow!("insert: at least one source file is required"));
    }
    let multi = sources.len() > 1 || as_directory;
    let dest_in_pkg = session.resolve_package_path(dest)?;

    if multi {
        fs::create_dir_all(&dest_in_pkg)
            .with_context(|| format!("Creating destination directory {}", dest_in_pkg.display()))?;
        for src in sources {
            if !src.is_file() {
                return Err(anyhow!("Source file not found: {}", src.display()));
            }
            let target = dest_in_pkg.join(
                src.file_name()
                    .ok_or_else(|| anyhow!("Source has no filename: {}", src.display()))?,
            );
            log::info!("insert {} → {}", src.display(), target.display());
            fs::copy(src, &target)
                .with_context(|| format!("Copying {} → {}", src.display(), target.display()))?;
        }
    } else {
        let src = &sources[0];
        if !src.is_file() {
            return Err(anyhow!("Source file not found: {}", src.display()));
        }
        if let Some(parent) = dest_in_pkg.parent() {
            fs::create_dir_all(parent)?;
        }
        log::info!("insert {} → {}", src.display(), dest_in_pkg.display());
        fs::copy(src, &dest_in_pkg)
            .with_context(|| format!("Copying {} → {}", src.display(), dest_in_pkg.display()))?;
    }
    Ok(())
}

/// Remove every file inside the package matching `pattern` (a path with
/// optional glob characters: `*`, `?`, `[…]`). Returns the number removed.
pub fn remove(session: &Session, pattern: &str) -> Result<usize> {
    let matches = glob_in_data(session, pattern)?;
    if matches.is_empty() {
        log::info!("remove: no files matched {}", pattern);
        return Ok(0);
    }
    let mut count = 0usize;
    for path in &matches {
        log::info!("remove {}", path.display());
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
        count += 1;
    }
    Ok(count)
}

/// Move a file inside the package from `src` to `dest`. Creates parent
/// directories of `dest` if needed. Refuses to move a file outside the
/// session's `data/` directory.
pub fn move_path(session: &Session, src: &str, dest: &str) -> Result<()> {
    let src_path = session.resolve_package_path(src)?;
    let dest_path = session.resolve_package_path(dest)?;
    if !src_path.exists() {
        return Err(anyhow!("Source file not found in package: {}", src));
    }
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    log::info!("move {} → {}", src_path.display(), dest_path.display());
    fs::rename(&src_path, &dest_path)
        .with_context(|| format!("Renaming {} → {}", src_path.display(), dest_path.display()))?;
    Ok(())
}

/// Overwrite every file inside the package matching `pattern` with the
/// contents of `replacement`. Returns the number of files replaced.
pub fn replace(session: &Session, pattern: &str, replacement: &Path) -> Result<usize> {
    if !replacement.is_file() {
        return Err(anyhow!(
            "Replacement file not found: {}",
            replacement.display()
        ));
    }
    let matches = glob_in_data(session, pattern)?;
    if matches.is_empty() {
        log::info!("replace: no files matched {}", pattern);
        return Ok(0);
    }
    let mut count = 0usize;
    for path in &matches {
        if path.is_dir() {
            return Err(anyhow!(
                "Refusing to replace a directory with a file: {}",
                path.display()
            ));
        }
        log::info!("replace {} ← {}", path.display(), replacement.display());
        fs::copy(replacement, path)?;
        count += 1;
    }
    Ok(count)
}

/// Glob `pattern` (interpreted relative to the package root) against the
/// session's `data/` directory. Rejects patterns that would resolve
/// outside the session.
fn glob_in_data(session: &Session, pattern: &str) -> Result<Vec<PathBuf>> {
    let stripped = pattern.strip_prefix('/').unwrap_or(pattern);
    let full_pattern = session.data_dir().join(stripped);
    // Sanity-check that the *stem* of the pattern stays inside data/. We
    // can't normalize a pattern that contains `*`/`?` directly, so we
    // check the longest non-glob prefix.
    let stem: PathBuf = full_pattern
        .components()
        .take_while(|c| {
            let s = c.as_os_str().to_string_lossy();
            !s.contains('*') && !s.contains('?') && !s.contains('[')
        })
        .collect();
    if !stem.starts_with(session.data_dir()) {
        return Err(anyhow!(
            "Pattern escapes session data directory: {}",
            pattern
        ));
    }

    let s = full_pattern.to_string_lossy().to_string();
    let mut out = Vec::new();
    for entry in glob::glob(&s).with_context(|| format!("Globbing {}", s))? {
        match entry {
            Ok(p) => out.push(p),
            Err(e) => log::warn!("glob entry error: {}", e),
        }
    }
    Ok(out)
}
