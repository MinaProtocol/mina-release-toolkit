use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;

pub fn check_command_exists(cmd: &str) -> Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {} >/dev/null 2>&1", cmd))
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "Required program '{}' is not installed or not in PATH. `sudo apt-get install {}`",
            cmd,
            cmd
        ))
    }
}

pub fn check_file_exists(file: &str) -> Result<()> {
    if Path::new(file).exists() {
        Ok(())
    } else {
        Err(anyhow!(
            "File ({}) does not exist or permission denied",
            file
        ))
    }
}

pub fn download_file(url: &str, dest: &Path, debug: bool) -> Result<()> {
    if debug {
        log::info!("Executing: curl -s -o {} {}", dest.display(), url);
    }
    let output = Command::new("curl")
        .args(["-s", "-o"])
        .arg(dest)
        .arg(url)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "Failed to download file {} ({})",
            dest.display(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}
