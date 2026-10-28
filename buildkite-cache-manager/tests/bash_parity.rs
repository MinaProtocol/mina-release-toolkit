//! Behaviour the mina CI scripts rely on from the bash `manager.sh`
//! (`cp -R -f` under the hood). Runs the real binary against a temp cache.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const BUILD_ID: &str = "build-1";

struct Env {
    _tmp: TempDir,
    cache: PathBuf,
    local: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let local = tmp.path().join("local");
        fs::create_dir_all(cache.join(BUILD_ID)).unwrap();
        fs::create_dir_all(&local).unwrap();
        Env {
            _tmp: tmp,
            cache,
            local,
        }
    }

    fn run(&self, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_buildkite-cache-manager"));
        cmd.args(args)
            .env("CACHE_BASE_URL", &self.cache)
            .env("BUILDKITE_BUILD_ID", BUILD_ID)
            .env_remove("MINA_READ_CACHE_ROOT");
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn cached(&self, rel: &str) -> PathBuf {
        self.cache.join(BUILD_ID).join(rel)
    }
}

fn put(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn write_overwrites_existing_file_by_default() {
    let env = Env::new();
    let src = env.local.join("a.deb");
    put(&src, "v1");
    assert!(env
        .run(&["write", s(&src), "debians/"], &[])
        .status
        .success());
    put(&src, "v2");
    let out = env.run(&["write", s(&src), "debians/"], &[]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        fs::read_to_string(env.cached("debians/a.deb")).unwrap(),
        "v2"
    );
}

#[test]
fn read_overwrites_existing_local_file_by_default() {
    let env = Env::new();
    put(&env.cached("x/f"), "new");
    let dest = env.local.join("out");
    put(&dest.join("f"), "old");
    assert!(env.run(&["read", "x/f", s(&dest)], &[]).status.success());
    assert_eq!(fs::read_to_string(dest.join("f")).unwrap(), "new");
}

#[test]
fn no_clobber_keeps_existing_file() {
    let env = Env::new();
    put(&env.cached("x/f"), "new");
    let dest = env.local.join("out");
    put(&dest.join("f"), "old");
    let out = env.run(&["read", "--no-clobber", "x/f", s(&dest)], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(fs::read_to_string(dest.join("f")).unwrap(), "old");
}

#[test]
fn reading_a_directory_keeps_its_name() {
    // `cp -R cache/hardfork /workdir` -> /workdir/hardfork/...
    let env = Env::new();
    put(&env.cached("hardfork/new_config.json"), "c");
    put(&env.cached("hardfork/ledgers/l.tar.gz"), "l");
    let dest = env.local.join("workdir");
    assert!(env
        .run(&["read", "hardfork", s(&dest)], &[])
        .status
        .success());
    assert!(dest.join("hardfork/new_config.json").is_file());
    assert!(dest.join("hardfork/ledgers/l.tar.gz").is_file());
    assert!(!dest.join("new_config.json").exists());
}

#[test]
fn mid_path_glob_reads_into_one_folder_across_roots() {
    // read_all_from_cache.sh reads several roots into the same folder.
    let env = Env::new();
    put(&env.cache.join("legacy/debians/noble/a.deb"), "legacy");
    put(&env.cached("debians/noble/a.deb"), "build");
    let dest = env.local.join("debs");
    assert!(env
        .run(
            &["read", "--root", "legacy/debians", "noble/*", s(&dest)],
            &[]
        )
        .status
        .success());
    assert!(env
        .run(&["read", "debians/noble/*", s(&dest)], &[])
        .status
        .success());
    assert_eq!(fs::read_to_string(dest.join("a.deb")).unwrap(), "build");
}

#[test]
fn glob_skips_dotfiles() {
    let env = Env::new();
    put(&env.cached("d/.hidden"), "h");
    put(&env.cached("d/visible"), "v");
    let dest = env.local.join("out");
    assert!(env.run(&["read", "d/*", s(&dest)], &[]).status.success());
    assert!(dest.join("visible").is_file());
    assert!(!dest.join(".hidden").exists());
}

#[test]
fn read_honours_mina_read_cache_root() {
    let env = Env::new();
    put(&env.cache.join("older-build/archive.perf"), "old");
    let dest = env.local.join("out");
    let out = env.run(
        &["read", "archive.perf", s(&dest)],
        &[("MINA_READ_CACHE_ROOT", "older-build")],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        fs::read_to_string(dest.join("archive.perf")).unwrap(),
        "old"
    );
}

#[test]
fn root_flag_beats_mina_read_cache_root() {
    let env = Env::new();
    put(&env.cache.join("explicit/f"), "explicit");
    let dest = env.local.join("out");
    let out = env.run(
        &["read", "--root", "explicit", "f", s(&dest)],
        &[("MINA_READ_CACHE_ROOT", "older-build")],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(fs::read_to_string(dest.join("f")).unwrap(), "explicit");
}

#[test]
fn write_ignores_mina_read_cache_root() {
    let env = Env::new();
    let src = env.local.join("f");
    put(&src, "x");
    let out = env.run(
        &["write", s(&src), "out/"],
        &[("MINA_READ_CACHE_ROOT", "older-build")],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(env.cached("out/f").is_file());
    assert!(!env.cache.join("older-build").exists());
}

#[test]
fn missing_input_exits_2() {
    // CI retries a step on exit status 2.
    let env = Env::new();
    let out = env.run(&["read", "nope/*", s(&env.local)], &[]);
    assert_eq!(out.status.code(), Some(2));
    let out = env.run(&["write", "/does/not/exist", "dst/"], &[]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn missing_build_id_exits_1() {
    let env = Env::new();
    let out = Command::new(env!("CARGO_BIN_EXE_buildkite-cache-manager"))
        .args(["read", "f", s(&env.local)])
        .env("CACHE_BASE_URL", &env.cache)
        .env_remove("BUILDKITE_BUILD_ID")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn write_leaves_no_temp_files() {
    let env = Env::new();
    let src = env.local.join("f");
    put(&src, "x");
    assert!(env.run(&["write", s(&src), "out/"], &[]).status.success());
    let names: Vec<_> = fs::read_dir(env.cached("out"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["f"]);
}

#[test]
fn deprecated_override_flag_is_accepted() {
    let env = Env::new();
    let src = env.local.join("f");
    put(&src, "x");
    let out = env.run(&["write", "--override", s(&src), "out/"], &[]);
    assert!(out.status.success(), "{out:?}");
}

/// `cp` without `-p` creates the destination with the source's mode and then
/// leaves it alone: no `chmod` afterwards. The CI cache depends on that: it is
/// a CIFS mount with a forced owner, where a writer running as another uid may
/// create a file but may not `fchmod` it (which `std::fs::copy` does). An
/// executable source stays executable in the cache, as with `cp`.
#[cfg(unix)]
#[test]
fn write_creates_the_file_with_the_source_mode_like_cp() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::new();
    let src = env.local.join("rosetta-cli");
    put(&src, "#!/bin/sh\n");
    fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).unwrap();

    let out = env.run(&["write", s(&src), "tool-cache/"], &[]);
    assert!(out.status.success(), "{out:?}");

    let cached = env.cached("tool-cache/rosetta-cli");
    assert_eq!(fs::read_to_string(&cached).unwrap(), "#!/bin/sh\n");
    let mode = fs::metadata(&cached).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "cp keeps the source mode on a new file");
}
