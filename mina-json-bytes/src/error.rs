//! The errors of this tool.

use crate::token::Token;
use std::io;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

/// An error in the arguments for jq.
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("{option} is not supported: give JSON on stdin or with --slurpfile")]
    UnsupportedOption { option: String },
    #[error("{option} needs more values")]
    MissingValue { option: String },
    #[error("{argument}: jq would read this file without encoding it: use stdin or --slurpfile")]
    FileArgument { argument: String },
    #[error("no jq filter")]
    NoFilter,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The encoder cannot keep this input apart from its own tokens.
    #[error("the input already contains the text {}", Token::PREFIX)]
    PrefixInInput,
    #[error("{}: {source}", path.display())]
    File { path: PathBuf, source: Box<Error> },
    #[error("cannot start jq: {0}")]
    StartJq(#[source] io::Error),
    #[error(transparent)]
    Usage(#[from] UsageError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Error {
    /// Adds the file that caused this error.
    pub(crate) fn in_file(self, path: impl Into<PathBuf>) -> Self {
        Self::File {
            path: path.into(),
            source: Box::new(self),
        }
    }

    /// True if the reader of our output closed it.
    pub(crate) fn is_broken_pipe(&self) -> bool {
        matches!(self, Self::Io(error) if error.kind() == io::ErrorKind::BrokenPipe)
    }

    /// The process exit code: 2 for a usage error as in clap, 1 for a failure.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => 2,
            _ => 1,
        }
    }
}
