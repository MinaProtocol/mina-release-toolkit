//! End-to-end pipeline test:
//!   1. Build a .deb from the fixture build_dir
//!   2. Import the fixture GPG secret key, confirm its id
//!   3. `sign` the .deb with that key
//!   4. `lookup sign-key` to confirm the embedded key id
//!   5. `verify signature` using the fixture public key
//!
//! Tools required at runtime: fakeroot, dpkg-deb, debsigs, debsig-verify, gpg.
//! Skips with a printed note if any are missing.

use std::path::{Path, PathBuf};
use std::process::Command;

const EXPECTED_KEY_ID: &str = "40C7DD112EDB4CA9";

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("res")
}

fn have(cmd: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {} >/dev/null 2>&1", cmd))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn fs_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if ty.is_dir() {
                fs_copy(&from, &to)?;
            } else {
                std::fs::copy(&from, &to)?;
            }
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

fn import_secret_key(secret_key: &Path, gnupg_home: &Path) -> String {
    let out = Command::new("gpg")
        .env("GNUPGHOME", gnupg_home)
        .args([
            "--import",
            "--import-options",
            "import-show",
            "--with-colons",
        ])
        .arg(secret_key)
        .output()
        .expect("spawn gpg --import");
    assert!(
        out.status.success(),
        "gpg --import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    // sec:u:3072:1:40C7DD112EDB4CA9:...
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("sec:") {
            // fields are separated by ':' — keyid is field index 4
            if let Some(id) = rest.split(':').nth(3) {
                return id.to_string();
            }
        }
    }
    panic!("could not find sec: line in gpg --import output:\n{}", text);
}

#[test]
fn build_sign_verify_end_to_end() {
    for tool in ["fakeroot", "dpkg-deb", "debsigs", "debsig-verify", "gpg"] {
        if !have(tool) {
            eprintln!(
                "skipping build_sign_verify_end_to_end: {} not on PATH",
                tool
            );
            return;
        }
    }

    let tmp = tempfile::Builder::new()
        .prefix("deb-toolkit-sign-")
        .tempdir()
        .unwrap();

    let build_dir = tmp.path().join("build_dir");
    fs_copy(&fixtures_dir().join("build_dir"), &build_dir).unwrap();

    let output_dir = tmp.path().join("out");
    let defaults = fixtures_dir().join("defaults.json");
    let secret_key = fixtures_dir().join("secret-key.gpg");
    let public_key = fixtures_dir().join("public-key.gpg");
    let gnupg_home = tmp.path().join("gnupg");
    std::fs::create_dir_all(&gnupg_home).unwrap();
    // GPG complains noisily if the homedir is world-readable.
    let mut perms = std::fs::metadata(&gnupg_home).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o700);
    std::fs::set_permissions(&gnupg_home, perms).unwrap();

    let bin = env!("CARGO_BIN_EXE_deb-toolkit");

    // ---- build ----
    let status = Command::new(bin)
        .args(["build"])
        .args(["--build-dir", build_dir.to_str().unwrap()])
        .args(["--output-dir", output_dir.to_str().unwrap()])
        .args(["--package-name", "example-app"])
        .args(["--version", "1.0.0"])
        .args(["--suite", "stable"])
        .args(["--codename", "focal"])
        .args(["--description", "example app"])
        .args(["--defaults-file", defaults.to_str().unwrap()])
        .status()
        .expect("spawn build");
    assert!(status.success(), "build failed");
    let deb = output_dir.join("example-app_1.0.0.deb");
    assert!(deb.exists());

    // ---- import key (also makes debsigs use the test home) ----
    let key_id = import_secret_key(&secret_key, &gnupg_home);
    assert_eq!(key_id, EXPECTED_KEY_ID);

    // ---- sign ----
    let status = Command::new(bin)
        .env("GNUPGHOME", &gnupg_home)
        .args(["sign"])
        .args(["--deb", deb.to_str().unwrap()])
        .args(["--key", &key_id])
        .status()
        .expect("spawn sign");
    assert!(status.success(), "sign failed");

    // ---- lookup sign-key ----
    let out = Command::new(bin)
        .args(["lookup", "sign-key", deb.to_str().unwrap()])
        .output()
        .expect("spawn lookup sign-key");
    assert!(
        out.status.success(),
        "lookup sign-key failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed.trim() == EXPECTED_KEY_ID,
        "expected printed key id {}, got: {:?}",
        EXPECTED_KEY_ID,
        printed
    );

    // ---- verify signature ----
    let out = Command::new(bin)
        .args(["verify", "signature"])
        .arg(deb.to_str().unwrap())
        .args(["--key", public_key.to_str().unwrap()])
        .output()
        .expect("spawn verify signature");
    assert!(
        out.status.success(),
        "verify signature failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
