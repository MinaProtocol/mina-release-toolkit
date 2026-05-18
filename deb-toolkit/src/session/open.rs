use anyhow::{anyhow, Context, Result};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use super::compression::Compression;
use super::metadata::Metadata;
use super::Session;
use crate::misc::check_file_exists;

/// Open `input_deb` into a fresh session directory. If `session_dir` already
/// exists it is cleared first.
pub fn open(input_deb: &Path, session_dir: &Path) -> Result<Session> {
    check_file_exists(input_deb.to_str().unwrap_or(""))
        .map_err(|_| anyhow!("Input .deb file not found: {}", input_deb.display()))?;

    let input_deb_abs = input_deb
        .canonicalize()
        .with_context(|| format!("Could not resolve {}", input_deb.display()))?;

    // Create or clean session_dir. We refuse to follow symlinks to avoid
    // escaping the user's working tree.
    if session_dir.exists() {
        let meta = std::fs::symlink_metadata(session_dir)?;
        if meta.file_type().is_symlink() {
            return Err(anyhow!(
                "Session directory cannot be a symlink (security restriction): {}",
                session_dir.display()
            ));
        }
        for entry in fs::read_dir(session_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() && !entry.file_type()?.is_symlink() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        }
    } else {
        fs::create_dir_all(session_dir)?;
    }
    let session_dir_abs = session_dir.canonicalize()?;

    log::info!("=== Opening Debian Package ===");
    log::info!("Input:   {}", input_deb_abs.display());
    log::info!("Session: {}", session_dir_abs.display());

    // ---- extract outer .deb (ar) ----
    let deb_bytes = fs::read(&input_deb_abs)?;
    let mut ar_reader = ar::Archive::new(Cursor::new(&deb_bytes));

    let mut control_tar_bytes: Option<(String, Vec<u8>)> = None;
    let mut data_tar_bytes: Option<(String, Vec<u8>)> = None;
    let mut debian_binary_bytes: Option<Vec<u8>> = None;

    while let Some(entry) = ar_reader.next_entry() {
        let mut entry = entry?;
        let name = String::from_utf8_lossy(entry.header().identifier()).to_string();
        let mut buf = Vec::with_capacity(entry.header().size() as usize);
        entry.read_to_end(&mut buf)?;
        if name == "debian-binary" {
            debian_binary_bytes = Some(buf);
        } else if name.starts_with("control.tar") {
            control_tar_bytes = Some((name, buf));
        } else if name.starts_with("data.tar") {
            data_tar_bytes = Some((name, buf));
        }
    }

    let debian_binary = debian_binary_bytes
        .ok_or_else(|| anyhow!("debian-binary missing in {}", input_deb_abs.display()))?;
    let (control_name, control_bytes) = control_tar_bytes
        .ok_or_else(|| anyhow!("control.tar.* missing in {}", input_deb_abs.display()))?;
    let (data_name, data_bytes) = data_tar_bytes
        .ok_or_else(|| anyhow!("data.tar.* missing in {}", input_deb_abs.display()))?;

    let control_compress = Compression::from_filename(&control_name)?;
    let data_compress = Compression::from_filename(&data_name)?;

    log::info!("Found control archive: {}", control_name);
    log::info!("Found data archive:    {}", data_name);

    // Persist debian-binary as-is (it's literally the string "2.0\n").
    fs::write(session_dir_abs.join("debian-binary"), &debian_binary)?;

    // Decompress control archive, extract into control/.
    let control_dir = session_dir_abs.join("control");
    fs::create_dir_all(&control_dir)?;
    let mut control_tar = Vec::new();
    control_compress.decompress(Cursor::new(&control_bytes), &mut control_tar)?;
    extract_tar(&control_tar, &control_dir)?;

    // Decompress data archive, extract into data/.
    let data_dir = session_dir_abs.join("data");
    fs::create_dir_all(&data_dir)?;
    let mut data_tar = Vec::new();
    data_compress.decompress(Cursor::new(&data_bytes), &mut data_tar)?;
    extract_tar(&data_tar, &data_dir)?;

    // Pull the original Package/Version/Architecture fields out of the control
    // file we just extracted so they end up in metadata.env.
    let control_file = control_dir.join("control");
    let (pkg_name, pkg_version, pkg_arch) = read_pkg_meta(&control_file);

    let metadata = Metadata {
        input_deb: input_deb_abs,
        session_dir: session_dir_abs.clone(),
        data_compress,
        control_compress,
        created_at: chrono_now_utc(),
        package_name: pkg_name,
        package_version: pkg_version,
        package_arch: pkg_arch,
    };
    metadata.save(&session_dir_abs.join("metadata.env"))?;

    log::info!("=== Session Opened Successfully ===");
    Ok(Session {
        dir: session_dir_abs,
        metadata,
    })
}

fn extract_tar(tar_bytes: &[u8], dest: &PathBuf) -> Result<()> {
    let mut ar = tar::Archive::new(Cursor::new(tar_bytes));
    ar.set_preserve_permissions(true);
    ar.set_overwrite(true);
    ar.unpack(dest)
        .with_context(|| format!("Extracting tar into {}", dest.display()))?;
    Ok(())
}

fn read_pkg_meta(control_file: &Path) -> (Option<String>, Option<String>, Option<String>) {
    let text = match fs::read_to_string(control_file) {
        Ok(t) => t,
        Err(_) => return (None, None, None),
    };
    let mut pkg = None;
    let mut ver = None;
    let mut arch = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Package:") {
            pkg = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Version:") {
            ver = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Architecture:") {
            arch = Some(rest.trim().to_string());
        }
    }
    (pkg, ver, arch)
}

/// `2026-05-17T00:00:00Z`-ish UTC timestamp. Avoid pulling in chrono just
/// for this — `SystemTime` + simple math is enough.
fn chrono_now_utc() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Days from epoch
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let (h, m, s) = (
        time_of_day / 3600,
        (time_of_day / 60) % 60,
        time_of_day % 60,
    );
    let (y, mo, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, m, s)
}

/// Algorithm from Howard Hinnant — days since 1970-01-01 → (year, month, day).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
