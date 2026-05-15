//! End-to-end build test mirroring `src/test/test_deb_builder.ml`.
//!
//! Requires `fakeroot` and `dpkg-deb` to be available on PATH; the test is
//! skipped (with a printed note) on systems without them.

use std::path::PathBuf;
use std::process::Command;

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

#[test]
fn build_produces_deb() {
    if !have("fakeroot") || !have("dpkg-deb") {
        eprintln!("skipping build_produces_deb: fakeroot/dpkg-deb not on PATH");
        return;
    }

    let tmp = tempfile::Builder::new()
        .prefix("deb-builder-build-")
        .tempdir()
        .unwrap();

    // dpkg-deb requires a writable build_dir (it writes DEBIAN/control inside).
    // Copy the fixture build_dir into a temp location.
    let build_dir = tmp.path().join("build_dir");
    fs_extra_copy(&fixtures_dir().join("build_dir"), &build_dir).unwrap();

    let output_dir = tmp.path().join("out");
    let defaults = fixtures_dir().join("defaults.json");

    let bin = env!("CARGO_BIN_EXE_deb-builder");
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
        .expect("spawn deb-builder build");

    assert!(status.success(), "build subcommand failed");

    let deb = output_dir.join("example-app_1.0.0.deb");
    assert!(deb.exists(), "expected .deb at {}", deb.display());

    // Sanity-check the produced .deb with dpkg-deb -I.
    let out = Command::new("dpkg-deb")
        .args(["-I", deb.to_str().unwrap()])
        .output()
        .expect("spawn dpkg-deb -I");
    assert!(out.status.success(), "dpkg-deb -I exited non-zero");
    let info = String::from_utf8_lossy(&out.stdout);
    assert!(info.contains("Package: example-app"), "info:\n{}", info);
    assert!(info.contains("Version: 1.0.0"), "info:\n{}", info);
    assert!(info.contains("Architecture: amd64"), "info:\n{}", info);
    assert!(
        info.contains("Built from fakehash by localhost"),
        "info:\n{}",
        info
    );
}

// Tiny recursive copy so we don't pull in fs_extra just for one test.
fn fs_extra_copy(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if ty.is_dir() {
                fs_extra_copy(&from, &to)?;
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
