//! `mina-json-bytes jq`: run jq with the input encoded and the output decoded.
//!
//! jq opens its file arguments itself, so an input file given to jq directly
//! does not go through [`encode`](crate::encode). To make that impossible, the
//! arguments are checked here: JSON comes from stdin or from `--slurpfile`,
//! which is encoded to a temporary file, and any other file input is an error.

use crate::{decode, encode};
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File};
use std::io::{self, BufWriter};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

/// The options that take values, with the number of values.
const VALUE_OPTIONS: &[(&str, usize)] =
    &[("--arg", 2), ("--argjson", 2), ("--indent", 1), ("-L", 1)];

/// The options that give jq input which this wrapper cannot encode.
const REJECTED_OPTIONS: &[&str] = &["--args", "--jsonargs", "--from-file", "--rawfile", "--"];

#[derive(Debug, PartialEq)]
pub struct Plan {
    pub args: Vec<OsString>,
    /// Indexes in `args` of the files to encode.
    pub files: Vec<usize>,
    pub null_input: bool,
}

/// Checks the jq arguments and finds the files to encode.
pub fn plan(args: &[OsString]) -> Result<Plan, String> {
    let mut plan = Plan {
        args: Vec::new(),
        files: Vec::new(),
        null_input: false,
    };
    let mut have_filter = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        plan.args.push(arg.clone());
        let text = arg.to_string_lossy();
        if text == "--slurpfile" {
            plan.args
                .push(value(&mut iter, "--slurpfile")?.to_os_string());
            plan.args
                .push(value(&mut iter, "--slurpfile")?.to_os_string());
            plan.files.push(plan.args.len() - 1);
        } else if let Some((name, count)) = VALUE_OPTIONS.iter().find(|(name, _)| *name == text) {
            for _ in 0..*count {
                plan.args.push(value(&mut iter, name)?.to_os_string());
            }
        } else if REJECTED_OPTIONS.contains(&text.as_ref()) {
            return Err(format!("{text} is not supported: use stdin or --slurpfile"));
        } else if let Some(long) = text.strip_prefix("--") {
            plan.null_input |= long == "null-input";
        } else if let Some(short) = text.strip_prefix('-').filter(|short| !short.is_empty()) {
            if short.contains('f') {
                return Err(format!("{text} is not supported: give the filter here"));
            }
            plan.null_input |= short.contains('n');
        } else if !have_filter {
            have_filter = true;
        } else {
            return Err(format!(
                "{text}: jq would read this file without encoding it: \
                 use stdin or --slurpfile"
            ));
        }
    }
    if have_filter {
        Ok(plan)
    } else {
        Err("no jq filter".to_string())
    }
}

fn value<'a>(iter: &mut std::slice::Iter<'a, OsString>, option: &str) -> Result<&'a OsStr, String> {
    iter.next()
        .map(OsString::as_os_str)
        .ok_or_else(|| format!("{option} needs more values"))
}

/// A private directory for the encoded files, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> io::Result<Self> {
        let parent = std::env::temp_dir();
        let pid = std::process::id();
        for attempt in 0..100 {
            let path = parent.join(format!("mina-json-bytes.{pid}.{attempt}"));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other("cannot create a temporary directory"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn with_path(context: &OsStr, error: io::Error) -> io::Error {
    let name = String::from_utf8_lossy(context.as_bytes()).into_owned();
    io::Error::new(error.kind(), format!("{name}: {error}"))
}

/// Runs jq and returns its exit code.
pub fn run(mut plan: Plan) -> io::Result<i32> {
    let temp = TempDir::new()?;
    for (number, &index) in plan.files.iter().enumerate() {
        let source = plan.args[index].clone();
        let target = temp.path().join(format!("{number}.json"));
        let input = File::open(&source).map_err(|error| with_path(&source, error))?;
        let output = BufWriter::new(File::create(&target)?);
        encode(input, output).map_err(|error| with_path(&source, error))?;
        plan.args[index] = target.into_os_string();
    }

    // With --null-input jq does not need stdin, which can stay open forever.
    let stdin = if plan.null_input {
        Stdio::null()
    } else {
        Stdio::piped()
    };
    let mut child = Command::new("jq")
        .args(&plan.args)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| io::Error::new(error.kind(), format!("cannot start jq: {error}")))?;

    let child_stdin = child.stdin.take();
    let child_stdout = child.stdout.take().expect("stdout is piped");
    let encoder = thread::spawn(move || match child_stdin {
        Some(child_stdin) => match encode(io::stdin().lock(), child_stdin) {
            // jq stopped reading. Its exit code tells the result.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            result => result,
        },
        None => Ok(()),
    });

    let decoded = decode(child_stdout, io::stdout().lock());
    let status = child.wait()?;
    encoder.join().expect("the encoder thread does not panic")?;
    decoded?;
    Ok(status.code().unwrap_or(1))
}
