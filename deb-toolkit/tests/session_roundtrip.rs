//! End-to-end roundtrip test for the session subsystem:
//!   1. Build a tiny fixture .deb with `dpkg-deb`.
//!   2. `session open` it.
//!   3. Mutate it (rename-package, reversion --update-deps, replace-suite,
//!      insert/remove/move/replace, read-field).
//!   4. `session save` (with --verify so `dpkg-deb --info` is run).
//!   5. Inspect with `dpkg-deb -I` and `dpkg-deb -c`; assert mutations took.
//!
//! Skipped when `dpkg-deb` isn't on PATH so it doesn't break dev boxes
//! without it.

use std::path::PathBuf;
use std::process::Command;

fn have(cmd: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {} >/dev/null 2>&1", cmd))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn dpkg_deb_info(deb: &PathBuf) -> String {
    let out = Command::new("dpkg-deb")
        .arg("--info")
        .arg(deb)
        .output()
        .unwrap();
    assert!(out.status.success(), "dpkg-deb --info failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn dpkg_deb_contents(deb: &PathBuf) -> String {
    let out = Command::new("dpkg-deb")
        .arg("-c")
        .arg(deb)
        .output()
        .unwrap();
    assert!(out.status.success(), "dpkg-deb -c failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn session_full_roundtrip() {
    if !have("dpkg-deb") {
        eprintln!("skipping session_full_roundtrip: dpkg-deb not on PATH");
        return;
    }

    // --- 1. build fixture .deb ---
    let tmp = tempfile::tempdir().unwrap();
    let pkg_root = tmp.path().join("pkg");
    std::fs::create_dir_all(pkg_root.join("DEBIAN")).unwrap();
    std::fs::create_dir_all(pkg_root.join("usr/share/test/configs")).unwrap();
    std::fs::write(
        pkg_root.join("DEBIAN/control"),
        "Package: deb-toolkit-session-fixture\n\
         Version: 1.0.0\n\
         Architecture: amd64\n\
         Maintainer: test@example.com\n\
         Suite: unstable\n\
         Depends: libfoo (= 1.0.0), libbar (>= 1.0.0), libbaz\n\
         Description: fixture for the roundtrip test\n",
    )
    .unwrap();
    std::fs::write(
        pkg_root.join("usr/share/test/configs/config_devnet.json"),
        "{\"network\": \"devnet\"}\n",
    )
    .unwrap();
    std::fs::write(
        pkg_root.join("usr/share/test/configs/config_mainnet.json"),
        "{\"network\": \"mainnet\"}\n",
    )
    .unwrap();
    std::fs::write(pkg_root.join("usr/share/test/keep-me.txt"), "keep this\n").unwrap();

    let input_deb = tmp.path().join("fixture_1.0.0_amd64.deb");
    let out = Command::new("dpkg-deb")
        .args(["-Zgzip", "--build"])
        .arg(&pkg_root)
        .arg(&input_deb)
        .output()
        .expect("dpkg-deb");
    assert!(
        out.status.success(),
        "dpkg-deb --build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // --- 2. session open ---
    let session_dir = tmp.path().join("session");
    let bin = env!("CARGO_BIN_EXE_deb-toolkit");
    let status = Command::new(bin)
        .args(["session", "open"])
        .arg(&input_deb)
        .arg(&session_dir)
        .status()
        .expect("session open");
    assert!(status.success(), "session open failed");
    assert!(session_dir.join("metadata.env").is_file());
    assert!(session_dir.join("control/control").is_file());
    assert!(session_dir.join("data").is_dir());

    // --- 3. mutations ---

    // read-field smoke test
    let read = Command::new(bin)
        .args(["session", "read-field"])
        .arg(&session_dir)
        .arg("Package")
        .output()
        .unwrap();
    assert!(read.status.success());
    assert_eq!(
        String::from_utf8_lossy(&read.stdout).trim(),
        "deb-toolkit-session-fixture"
    );

    // rename-package
    let st = Command::new(bin)
        .args(["session", "rename-package"])
        .arg(&session_dir)
        .arg("deb-toolkit-renamed")
        .status()
        .unwrap();
    assert!(st.success());

    // replace-suite
    let st = Command::new(bin)
        .args(["session", "replace-suite"])
        .arg(&session_dir)
        .arg("stable")
        .status()
        .unwrap();
    assert!(st.success());

    // reversion --update-deps
    let st = Command::new(bin)
        .args(["session", "reversion"])
        .arg(&session_dir)
        .arg("2.0.0")
        .arg("--update-deps")
        .status()
        .unwrap();
    assert!(st.success());

    // insert (single)
    let new_file = tmp.path().join("ledger.tar.gz");
    std::fs::write(&new_file, b"fake ledger contents\n").unwrap();
    let st = Command::new(bin)
        .args(["session", "insert"])
        .arg(&session_dir)
        .arg("/var/lib/coda/ledger.tar.gz")
        .arg(&new_file)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(session_dir
        .join("data/var/lib/coda/ledger.tar.gz")
        .is_file());

    // insert -d (multi → directory)
    let lf1 = tmp.path().join("ledger1.tar.gz");
    let lf2 = tmp.path().join("ledger2.tar.gz");
    std::fs::write(&lf1, b"l1\n").unwrap();
    std::fs::write(&lf2, b"l2\n").unwrap();
    let st = Command::new(bin)
        .args(["session", "insert", "-d"])
        .arg(&session_dir)
        .arg("/var/lib/coda")
        .arg(&lf1)
        .arg(&lf2)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(session_dir
        .join("data/var/lib/coda/ledger1.tar.gz")
        .is_file());

    // replace via glob
    let new_config = tmp.path().join("new_config.json");
    std::fs::write(&new_config, b"{\"new\": true}\n").unwrap();
    let st = Command::new(bin)
        .args(["session", "replace"])
        .arg(&session_dir)
        .arg("/usr/share/test/configs/config_*.json")
        .arg(&new_config)
        .status()
        .unwrap();
    assert!(st.success(), "replace failed");
    let content =
        std::fs::read_to_string(session_dir.join("data/usr/share/test/configs/config_devnet.json"))
            .unwrap();
    assert!(content.contains("\"new\": true"));

    // move
    let st = Command::new(bin)
        .args(["session", "move"])
        .arg(&session_dir)
        .arg("/usr/share/test/keep-me.txt")
        .arg("/usr/share/test/moved.txt")
        .status()
        .unwrap();
    assert!(st.success());
    assert!(!session_dir.join("data/usr/share/test/keep-me.txt").exists());
    assert!(session_dir.join("data/usr/share/test/moved.txt").is_file());

    // remove via glob (removes both config_*.json)
    let st = Command::new(bin)
        .args(["session", "remove"])
        .arg(&session_dir)
        .arg("/usr/share/test/configs/config_*.json")
        .status()
        .unwrap();
    assert!(st.success());
    assert!(!session_dir
        .join("data/usr/share/test/configs/config_devnet.json")
        .exists());

    // --- 4. session save --verify ---
    let output_deb = tmp.path().join("output.deb");
    let st = Command::new(bin)
        .args(["session", "save"])
        .arg(&session_dir)
        .arg(&output_deb)
        .arg("--verify")
        .status()
        .unwrap();
    assert!(st.success(), "session save --verify failed");
    assert!(output_deb.is_file());

    // --- 5. assertions on the produced .deb ---
    let info = dpkg_deb_info(&output_deb);
    assert!(
        info.contains("Package: deb-toolkit-renamed"),
        "Package not renamed:\n{}",
        info
    );
    assert!(
        info.contains("Version: 2.0.0"),
        "Version not bumped:\n{}",
        info
    );
    assert!(info.contains("Suite: stable"), "Suite not set:\n{}", info);
    assert!(
        info.contains("libfoo (= 2.0.0)"),
        "Depends not rewritten for =:\n{}",
        info
    );
    assert!(
        info.contains("libbar (>= 1.0.0)"),
        "Unrelated version was rewritten:\n{}",
        info
    );

    let contents = dpkg_deb_contents(&output_deb);
    assert!(
        contents.contains("var/lib/coda/ledger.tar.gz"),
        "{}",
        contents
    );
    assert!(
        contents.contains("var/lib/coda/ledger1.tar.gz"),
        "{}",
        contents
    );
    assert!(
        contents.contains("usr/share/test/moved.txt"),
        "{}",
        contents
    );
    assert!(
        !contents.contains("usr/share/test/keep-me.txt"),
        "{}",
        contents
    );
    assert!(
        !contents.contains("usr/share/test/configs/config_devnet.json"),
        "{}",
        contents
    );
}
