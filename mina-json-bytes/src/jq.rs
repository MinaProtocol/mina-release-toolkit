//! Run jq with the input encoded and the output decoded.
//!
//! jq opens its file arguments itself, so an input file given to jq directly
//! does not go through the [`Encoder`]. To make that impossible, each argument
//! is checked against [`RULES`]: JSON comes from stdin or from `--slurpfile`,
//! which is encoded to a temporary file, and any other file input is an error.

use crate::error::{Error, Result, UsageError};
use crate::{Decoder, Encoder, Transcode};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::slice;
use std::thread::{self, JoinHandle};

/// What a jq option means for this wrapper.
#[derive(Clone, Copy)]
enum OptionKind {
    /// Takes this number of values, none of them a JSON input.
    Values(usize),
    /// Takes a name and a JSON file, which must be encoded first.
    JsonFile,
    /// jq reads no JSON from stdin.
    NoStdin,
    /// Gives jq input that this wrapper cannot encode.
    Unsupported,
}

/// How to handle one jq option. An option with no rule takes no value.
struct OptionRule {
    name: &'static str,
    kind: OptionKind,
}

impl OptionRule {
    const fn new(name: &'static str, kind: OptionKind) -> Self {
        Self { name, kind }
    }
}

const RULES: &[OptionRule] = &[
    OptionRule::new("--arg", OptionKind::Values(2)),
    OptionRule::new("--argjson", OptionKind::Values(2)),
    OptionRule::new("--indent", OptionKind::Values(1)),
    OptionRule::new("-L", OptionKind::Values(1)),
    OptionRule::new("--slurpfile", OptionKind::JsonFile),
    OptionRule::new("-n", OptionKind::NoStdin),
    OptionRule::new("--null-input", OptionKind::NoStdin),
    OptionRule::new("--rawfile", OptionKind::Unsupported),
    OptionRule::new("--args", OptionKind::Unsupported),
    OptionRule::new("--jsonargs", OptionKind::Unsupported),
    OptionRule::new("-f", OptionKind::Unsupported),
    OptionRule::new("--from-file", OptionKind::Unsupported),
    OptionRule::new("--", OptionKind::Unsupported),
];

/// The rules of an option. `-nc` has the rules of both `-n` and `-c`.
fn rules_of(option: &str) -> Vec<&'static OptionRule> {
    let names: Vec<String> = if option.starts_with("--") {
        vec![option.to_string()]
    } else {
        option.chars().skip(1).map(|c| format!("-{c}")).collect()
    };
    let has_rule = |rule: &&OptionRule| names.iter().any(|name| name == rule.name);
    RULES.iter().filter(has_rule).collect()
}

fn is_option(argument: &OsStr) -> bool {
    argument.len() > 1 && argument.as_encoded_bytes().starts_with(b"-")
}

/// One argument of the jq command.
#[derive(Debug, PartialEq)]
enum JqArgument {
    /// Passed to jq as it is.
    Plain(OsString),
    /// A JSON file, which jq gets in encoded form.
    JsonFile(PathBuf),
}

/// A checked jq command.
#[derive(Debug, Default, PartialEq)]
pub struct JqInvocation {
    arguments: Vec<JqArgument>,
    /// True if jq reads no JSON from stdin (`--null-input`). stdin is then not
    /// connected, because it can stay open forever.
    no_stdin: bool,
    has_filter: bool,
}

impl JqInvocation {
    /// Checks the jq arguments.
    pub fn parse(arguments: &[OsString]) -> Result<Self> {
        let mut invocation = Self::default();
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            if is_option(argument) {
                invocation.add_option(argument, &mut arguments)?;
            } else {
                invocation.add_positional(argument)?;
            }
        }
        if invocation.has_filter {
            Ok(invocation)
        } else {
            Err(UsageError::NoFilter.into())
        }
    }

    fn add_plain(&mut self, argument: &OsStr) {
        self.arguments.push(JqArgument::Plain(argument.to_owned()));
    }

    /// The only positional argument is the filter, because any other one is a
    /// file that jq would read without encoding.
    fn add_positional(&mut self, argument: &OsStr) -> Result<()> {
        if self.has_filter {
            let argument = argument.to_string_lossy().into_owned();
            return Err(UsageError::FileArgument { argument }.into());
        }
        self.has_filter = true;
        self.add_plain(argument);
        Ok(())
    }

    fn add_option(
        &mut self,
        option: &OsStr,
        arguments: &mut slice::Iter<'_, OsString>,
    ) -> Result<()> {
        self.add_plain(option);
        let text = option.to_string_lossy();
        for rule in rules_of(&text) {
            self.apply(rule, arguments)?;
        }
        Ok(())
    }

    fn apply(
        &mut self,
        rule: &OptionRule,
        arguments: &mut slice::Iter<'_, OsString>,
    ) -> Result<()> {
        let mut value = || {
            let option = rule.name.to_string();
            arguments.next().ok_or(UsageError::MissingValue { option })
        };
        match rule.kind {
            OptionKind::Values(count) => {
                for _ in 0..count {
                    let plain = value()?;
                    self.add_plain(plain);
                }
            }
            OptionKind::JsonFile => {
                let name = value()?;
                let file = value()?;
                self.add_plain(name);
                self.arguments.push(JqArgument::JsonFile(file.into()));
            }
            OptionKind::NoStdin => self.no_stdin = true,
            OptionKind::Unsupported => {
                let option = rule.name.to_string();
                return Err(UsageError::UnsupportedOption { option }.into());
            }
        }
        Ok(())
    }

    /// Runs jq, with its output on stdout, and returns its exit code.
    pub fn run(&self) -> Result<u8> {
        let encoded_files = tempfile::Builder::new()
            .prefix("mina-json-bytes")
            .tempdir()?;
        let arguments = self.encoded_arguments(encoded_files.path())?;
        let mut jq = self.spawn(&arguments)?;
        let feeder = feed_stdin(jq.stdin.take());
        let stdout = jq.stdout.take().expect("stdout is piped");
        let decoded = Decoder::new(io::stdout().lock()).run(stdout);
        let status = jq.wait()?;
        feeder.join().expect("the feeder thread does not panic")?;
        decoded?;
        let code = status.code().unwrap_or(1);
        Ok(u8::try_from(code).unwrap_or(1))
    }

    /// The arguments for jq, after encoding each JSON file into `directory`.
    fn encoded_arguments(&self, directory: &Path) -> Result<Vec<OsString>> {
        let encode = |(index, argument): (usize, &JqArgument)| match argument {
            JqArgument::Plain(plain) => Ok(plain.clone()),
            JqArgument::JsonFile(file) => {
                let target = directory.join(format!("{index}.json"));
                encode_file(file, &target).map_err(|error| error.in_file(file))?;
                Ok(target.into_os_string())
            }
        };
        self.arguments.iter().enumerate().map(encode).collect()
    }

    fn spawn(&self, arguments: &[OsString]) -> Result<Child> {
        let stdin = if self.no_stdin {
            Stdio::null()
        } else {
            Stdio::piped()
        };
        Command::new("jq")
            .args(arguments)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .spawn()
            .map_err(Error::StartJq)
    }
}

fn encode_file(source: &Path, target: &Path) -> Result<()> {
    Encoder::new(File::create(target)?).run(File::open(source)?)
}

/// Encodes our stdin into the stdin of jq, in a thread: the main thread reads
/// the output of jq at the same time, so that the two pipes cannot block.
fn feed_stdin(jq_stdin: Option<ChildStdin>) -> JoinHandle<Result<()>> {
    thread::spawn(move || {
        let Some(jq_stdin) = jq_stdin else {
            return Ok(());
        };
        match Encoder::new(jq_stdin).run(io::stdin().lock()) {
            // jq stopped reading. Its exit code tells the result.
            Err(error) if error.is_broken_pipe() => Ok(()),
            result => result,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<JqInvocation> {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        JqInvocation::parse(&arguments)
    }

    fn is_usage_error(arguments: &[&str]) -> bool {
        matches!(parse(arguments), Err(Error::Usage(_)))
    }

    fn plain(text: &str) -> JqArgument {
        JqArgument::Plain(text.into())
    }

    #[test]
    fn values_are_not_taken_for_the_filter_or_a_file() {
        let parsed = parse(&["--arg", "a", "b", "--argjson", "c", "1", "."]).unwrap();
        assert_eq!(parsed.arguments.len(), 7);
        assert!(!parsed.no_stdin);
    }

    #[test]
    fn slurpfile_keeps_its_name_and_marks_its_file() {
        let parsed = parse(&["-n", "--slurpfile", "name", "file.json", "$name"]).unwrap();
        assert_eq!(
            parsed.arguments,
            [
                plain("-n"),
                plain("--slurpfile"),
                plain("name"),
                JqArgument::JsonFile("file.json".into()),
                plain("$name"),
            ]
        );
    }

    #[test]
    fn null_input_in_any_form_needs_no_stdin() {
        for option in ["-n", "--null-input", "-nc", "-cn"] {
            assert!(parse(&[option, "."]).unwrap().no_stdin, "{option}");
        }
        assert!(!parse(&["-c", "."]).unwrap().no_stdin);
    }

    #[test]
    fn inputs_that_cannot_be_encoded_are_usage_errors() {
        for arguments in [
            &[".", "ledger.json"][..],
            &["--rawfile", "a", "ledger.json", "."],
            &["--args", ".", "a"],
            &["--jsonargs", ".", "a"],
            &["-f", "filter.jq"],
            &["-cf", "filter.jq"],
            &["--from-file", "filter.jq"],
            &["--", "."],
        ] {
            assert!(is_usage_error(arguments), "{arguments:?}");
        }
    }

    #[test]
    fn missing_parts_are_usage_errors() {
        assert!(is_usage_error(&[]));
        assert!(is_usage_error(&["-c"]));
        assert!(is_usage_error(&["--arg", "a"]));
        assert!(is_usage_error(&["--slurpfile", "a"]));
    }
}
