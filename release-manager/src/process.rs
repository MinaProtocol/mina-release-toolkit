//! Thin abstraction over `std::process::Command` so subcommands that shell
//! out to `deb-s3`, `aws`, `dig`, `docker`, etc. can be unit-tested with a
//! fake executor instead of needing the real binaries.
//!
//! The real impl is the default; tests construct a [`MockCommandExecutor`]
//! and pre-register responses keyed by the program name.

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    #[cfg(test)]
    pub fn success(stdout: impl Into<String>) -> Self {
        CommandOutput {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    #[cfg(test)]
    pub fn failure(status: i32, stderr: impl Into<String>) -> Self {
        CommandOutput {
            status,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    pub fn is_success(&self) -> bool {
        self.status == 0
    }
}

pub trait CommandExecutor: Send + Sync {
    /// Run `program` with the given `args` and return its exit status,
    /// stdout, and stderr as a single `CommandOutput`. Implementations
    /// must not panic on non-zero exit — that's a legitimate result.
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutput>;
}

/// Production implementation — actually spawns the process.
pub struct RealExecutor;

impl CommandExecutor for RealExecutor {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutput> {
        let out = std::process::Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Test double. Pre-register `(program, predicate) -> output` rules.
///
/// On each call the mock walks its rules in registration order and returns
/// the first match. Unmatched calls return a `status=127` "command not
/// mocked" failure so tests fail loudly rather than silently succeeding.
/// Every call is recorded in `calls` for later assertions.
#[cfg(test)]
pub struct MockCommandExecutor {
    rules: Mutex<Vec<Rule>>,
    pub calls: Mutex<Vec<RecordedCall>>,
}

#[cfg(test)]
type ArgMatcher = Box<dyn Fn(&[&str]) -> bool + Send + Sync>;

#[cfg(test)]
struct Rule {
    program: String,
    matcher: ArgMatcher,
    output: CommandOutput,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct RecordedCall {
    pub program: String,
    pub args: Vec<String>,
}

#[cfg(test)]
impl Default for MockCommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl MockCommandExecutor {
    pub fn new() -> Self {
        Self {
            rules: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Register a rule: when `program` is invoked with args matching
    /// `matcher`, return `output`.
    pub fn expect<F>(&self, program: &str, matcher: F, output: CommandOutput)
    where
        F: Fn(&[&str]) -> bool + Send + Sync + 'static,
    {
        self.rules.lock().unwrap().push(Rule {
            program: program.to_string(),
            matcher: Box::new(matcher),
            output,
        });
    }

    /// Convenience: register a rule that matches by argv prefix.
    pub fn expect_args_starting_with(
        &self,
        program: &str,
        prefix: &[&'static str],
        output: CommandOutput,
    ) {
        let prefix: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
        self.expect(
            program,
            move |args| {
                args.len() >= prefix.len()
                    && args.iter().zip(prefix.iter()).all(|(a, p)| a == p)
            },
            output,
        );
    }

    /// How many times the executor was called for a given program.
    pub fn call_count(&self, program: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.program == program)
            .count()
    }
}

#[cfg(test)]
impl CommandExecutor for MockCommandExecutor {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutput> {
        self.calls.lock().unwrap().push(RecordedCall {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        });

        let rules = self.rules.lock().unwrap();
        for rule in rules.iter() {
            if rule.program == program && (rule.matcher)(args) {
                return Ok(rule.output.clone());
            }
        }
        // Unmatched: return a loud failure so tests notice.
        let argv: HashMap<usize, &str> =
            args.iter().enumerate().map(|(i, s)| (i, *s)).collect();
        Ok(CommandOutput {
            status: 127,
            stdout: String::new(),
            stderr: format!(
                "MockCommandExecutor: no rule matched {} {:?}",
                program, argv
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmatched_call_returns_127_and_records() {
        let mock = MockCommandExecutor::new();
        let out = mock.run("deb-s3", &["list", "--bucket=foo"]).unwrap();
        assert_eq!(out.status, 127);
        assert!(out.stderr.contains("no rule matched"));
        assert_eq!(mock.call_count("deb-s3"), 1);
    }

    #[test]
    fn matches_first_registered_rule() {
        let mock = MockCommandExecutor::new();
        mock.expect_args_starting_with(
            "deb-s3",
            &["list"],
            CommandOutput::success("pkg1 1.0 amd64"),
        );
        mock.expect_args_starting_with(
            "deb-s3",
            &["verify"],
            CommandOutput::success("ok"),
        );
        let out = mock.run("deb-s3", &["list", "--bucket=foo"]).unwrap();
        assert_eq!(out.stdout, "pkg1 1.0 amd64");
        let out = mock.run("deb-s3", &["verify", "--bucket=foo"]).unwrap();
        assert_eq!(out.stdout, "ok");
        assert_eq!(mock.call_count("deb-s3"), 2);
    }

    #[test]
    fn custom_matcher() {
        let mock = MockCommandExecutor::new();
        mock.expect(
            "aws",
            |args| args.contains(&"create-invalidation"),
            CommandOutput::success("{\"Invalidation\":{\"Id\":\"INV123\"}}"),
        );
        let out = mock
            .run(
                "aws",
                &["cloudfront", "create-invalidation", "--distribution-id", "X"],
            )
            .unwrap();
        assert!(out.stdout.contains("INV123"));
    }
}
