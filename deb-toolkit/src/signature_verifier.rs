use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;

use crate::misc::{check_command_exists, check_file_exists, download_file};
use crate::templates::{format_policy_file, PolicyFileInput};
use crate::viewer;

/// Build the per-key directory layout that debsig-verify expects:
///   <temp_dir>/<key_id>/key.gpg
///   <temp_dir>/<key_id>/policy.pol
/// Returns the path where the caller must place the public key.
fn build_verification_resources(temp_dir: &Path, key_id: &str) -> Result<std::path::PathBuf> {
    let key_id_dir = temp_dir.join(key_id);
    std::fs::create_dir_all(&key_id_dir)?;

    let key_filename = "key.gpg";
    let dest_key = key_id_dir.join(key_filename);

    let policy_file = key_id_dir.join("policy.pol");
    let policy_text = format_policy_file(&PolicyFileInput {
        key_filename,
        key_id,
        description: "deb",
    })?;
    std::fs::write(&policy_file, policy_text.as_bytes())?;
    log::debug!("Temporary policy file created at {}", policy_file.display());

    Ok(dest_key)
}

pub fn verify(deb: &str, public_key_file: Option<&str>, debug: bool) -> Result<()> {
    check_command_exists("debsig-verify")?;
    check_command_exists("curl")?;
    check_file_exists(deb)?;

    let temp = tempfile::Builder::new().prefix("debsig_verify").tempdir()?;
    let temp_dir = temp.path();

    let key_id = viewer::signature(deb, debug)?;
    let dest_key = build_verification_resources(temp_dir, &key_id)?;

    let mut cmd = Command::new("debsig-verify");
    cmd.arg("--policies-dir").arg(temp_dir);

    match public_key_file {
        None => {
            log::debug!(
                "No public key file provided. Key should reside in \
                 /usr/share/debsig/keyring/[key_id]/key.gpg"
            );
        }
        Some(public_key_file) => {
            let resolved: std::path::PathBuf = if public_key_file.starts_with("http://")
                || public_key_file.starts_with("https://")
            {
                let temp_file = temp_dir.join("downloaded_key.gpg");
                download_file(public_key_file, &temp_file, debug).map_err(|_| {
                    anyhow!(
                        "Failed to download public key file from URL {}",
                        public_key_file
                    )
                })?;
                log::debug!(
                    "Downloaded public key file from URL to {}",
                    temp_file.display()
                );
                temp_file
            } else {
                Path::new(public_key_file).to_path_buf()
            };

            log::debug!(
                "Public key file provided. Assuming policy and key resides in {}",
                resolved.display()
            );

            std::fs::copy(&resolved, &dest_key)?;
            log::debug!("Copied public key file to {}", dest_key.display());

            cmd.arg("--keyrings-dir").arg(temp_dir);
        }
    }

    cmd.arg("--debug").arg(deb);

    if debug {
        log::info!("Executing: {:?}", cmd);
    }

    let output = cmd
        .output()
        .map_err(|e| anyhow!("Failed to spawn debsig-verify: {}", e))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!("Failed to verify debian package {}. {}", deb, stdout);
    }

    Ok(())
}
