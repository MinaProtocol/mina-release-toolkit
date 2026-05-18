use anyhow::{anyhow, Context, Result};
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use super::Session;

/// Repack a session into `output_deb`. When `verify` is true, run a quick
/// sanity check with `dpkg-deb --info` if it is available on PATH.
pub fn save(session: &Session, output_deb: &Path, verify: bool) -> Result<()> {
    let output_abs = if output_deb.is_absolute() {
        output_deb.to_path_buf()
    } else {
        std::env::current_dir()?.join(output_deb)
    };

    log::info!("=== Saving Debian Package ===");
    log::info!("Session: {}", session.dir.display());
    log::info!("Output:  {}", output_abs.display());

    if !session.control_dir().is_dir() || !session.data_dir().is_dir() {
        return Err(anyhow!(
            "Session missing required directories (control/ or data/)"
        ));
    }

    // Build control.tar (with compression suffix), data.tar (likewise).
    let control_tar_name = format!("control.tar{}", session.metadata.control_compress.suffix());
    let data_tar_name = format!("data.tar{}", session.metadata.data_compress.suffix());

    let mut control_tar = Vec::new();
    pack_tar(&session.control_dir(), &mut control_tar)?;
    let mut control_compressed = Vec::new();
    session
        .metadata
        .control_compress
        .compress(Cursor::new(&control_tar), &mut control_compressed)?;

    let mut data_tar = Vec::new();
    pack_tar(&session.data_dir(), &mut data_tar)?;
    let mut data_compressed = Vec::new();
    session
        .metadata
        .data_compress
        .compress(Cursor::new(&data_tar), &mut data_compressed)?;

    // debian-binary is preserved verbatim from open(); fall back to "2.0\n"
    // if for some reason the session is missing it.
    let debian_binary = match fs::read(session.dir.join("debian-binary")) {
        Ok(b) if !b.is_empty() => b,
        _ => b"2.0\n".to_vec(),
    };

    // Assemble the outer .deb (ar archive: debian-binary, control, data).
    if let Some(parent) = output_abs.parent() {
        fs::create_dir_all(parent)?;
    }
    let output_file =
        File::create(&output_abs).with_context(|| format!("Creating {}", output_abs.display()))?;
    let mut ar_writer = ar::Builder::new(output_file);

    ar_append(&mut ar_writer, "debian-binary", &debian_binary)?;
    ar_append(&mut ar_writer, &control_tar_name, &control_compressed)?;
    ar_append(&mut ar_writer, &data_tar_name, &data_compressed)?;

    drop(ar_writer);

    if verify {
        log::info!("=== Verifying Package ===");
        verify_with_dpkg(&output_abs)?;
        log::info!("✓ Package verification passed");
    }

    log::info!("=== Package Saved Successfully ===");
    log::info!("Output: {}", output_abs.display());
    Ok(())
}

/// Recursively pack `dir` into a tar stream written to `out`. Files are
/// enumerated in sorted order for reproducibility, and every entry gets
/// uid=0/gid=0/mtime=0 plus empty user/group names.
fn pack_tar(dir: &Path, out: &mut Vec<u8>) -> Result<()> {
    let mut builder = tar::Builder::new(out);
    builder.mode(tar::HeaderMode::Deterministic);
    builder.follow_symlinks(false);

    // Collect a sorted list of (relative path, absolute path) so the tar
    // output is byte-stable across filesystems with different traversal
    // orders.
    let mut entries: Vec<(PathBuf, PathBuf)> = Vec::new();
    walk(dir, dir, &mut entries)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (rel, abs) in entries {
        let meta = fs::symlink_metadata(&abs)?;
        let mut header = tar::Header::new_gnu();
        header.set_uid(0);
        header.set_gid(0);
        header.set_username("")?;
        header.set_groupname("")?;
        header.set_mtime(0);
        // Strip mode group bits that vary across filesystems; keep just
        // the user-visible permissions.
        let mode = meta.permissions_from_unix();
        header.set_mode(mode);
        header.set_size(0);
        if meta.is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            // Tar dir entries traditionally end with `/`.
            let rel_with_slash = format!("{}/", rel.display());
            builder.append_data(&mut header, &rel_with_slash, std::io::empty())?;
        } else if meta.file_type().is_symlink() {
            header.set_entry_type(tar::EntryType::Symlink);
            let link = fs::read_link(&abs)?;
            builder.append_link(&mut header, &rel, &link)?;
        } else {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(meta.len());
            let mut f = File::open(&abs)?;
            builder.append_data(&mut header, &rel, &mut f)?;
        }
    }
    builder.finish()?;
    Ok(())
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let abs = entry.path();
        let rel = abs.strip_prefix(root).unwrap().to_path_buf();
        let ft = entry.file_type()?;
        if ft.is_dir() && !ft.is_symlink() {
            out.push((rel.clone(), abs.clone()));
            walk(root, &abs, out)?;
        } else {
            out.push((rel, abs));
        }
    }
    Ok(())
}

fn ar_append<W: Write>(builder: &mut ar::Builder<W>, name: &str, data: &[u8]) -> Result<()> {
    let mut header = ar::Header::new(name.as_bytes().to_vec(), data.len() as u64);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mode(0o100644);
    builder.append(&header, data)?;
    Ok(())
}

fn verify_with_dpkg(deb: &Path) -> Result<()> {
    use std::process::Command;
    let have_dpkg = Command::new("sh")
        .arg("-c")
        .arg("command -v dpkg-deb >/dev/null 2>&1")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !have_dpkg {
        log::info!("(dpkg-deb not on PATH; skipping verify)");
        return Ok(());
    }
    let out = Command::new("dpkg-deb").arg("--info").arg(deb).output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "Generated package is not a valid .deb file:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Package:")
            || trimmed.starts_with("Version:")
            || trimmed.starts_with("Architecture:")
        {
            log::info!("{}", line);
        }
    }
    Ok(())
}

/// Tiny helper because `Permissions::mode()` requires the unix extension.
trait PermissionsFromUnix {
    fn permissions_from_unix(&self) -> u32;
}

impl PermissionsFromUnix for fs::Metadata {
    fn permissions_from_unix(&self) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        self.permissions().mode()
    }
}
