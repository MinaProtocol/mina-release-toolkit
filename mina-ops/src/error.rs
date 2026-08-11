use thiserror::Error;

pub type OpsResult<T> = Result<T, OpsError>;

#[derive(Debug, Error)]
pub enum OpsError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("no Buildkite API token found. Set BUILDKITE_API_TOKEN (or BUILDKITE_API_ACCESS_TOKEN), or write the token to {0}")]
    MissingToken(String),

    #[error("Buildkite API error: {0}")]
    Buildkite(String),

    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("could not resolve a version: {0}")]
    UnresolvedVersion(String),

    #[error("{0}")]
    Other(String),
}
