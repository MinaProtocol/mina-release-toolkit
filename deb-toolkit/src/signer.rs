use anyhow::{anyhow, Result};
use std::process::Command;

use crate::misc::{check_command_exists, check_file_exists};

pub fn sign(deb: &str, signing_key_id: &str, debug: bool) -> Result<()> {
    check_command_exists("debsigs")?;
    check_file_exists(deb)?;

    log::info!("Signing package {} ...", deb);
    if debug {
        log::info!(
            "Executing: debsigs --sign=origin -k {} {}",
            signing_key_id,
            deb
        );
    }

    let output = Command::new("debsigs")
        .args(["--sign=origin", "-k", signing_key_id, deb])
        .output()
        .map_err(|e| anyhow!("Failed to spawn debsigs: {}", e))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!(
            "Failed to sign package {}. Stdout: {} , Stderr: {}",
            deb,
            stdout,
            stderr
        );
        anyhow::bail!("Failed to sign debian package {}", deb);
    }

    log::info!(
        "Package {} signed successfully using key {}",
        deb,
        signing_key_id
    );
    Ok(())
}
