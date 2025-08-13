use thiserror::Error;

pub type ManagerResult<T> = Result<T, ManagerError>;

#[derive(Error, Debug)]
pub enum ManagerError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    
    #[error("Command execution failed: {0}")]
    CommandFailed(String),
    
    #[error("Storage operation failed: {0}")]
    StorageError(String),
    
    #[error("Artifact not found: {0}")]
    ArtifactNotFound(String),
    
    
    #[error("Validation error: {0}")]
    ValidationError(String),
    
    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),
    
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
    
    #[error("Required parameter missing: {0}")]
    MissingParameter(String),
    
    #[error("Unsupported backend: {0}")]
    UnsupportedBackend(String),
    
    #[error("Unknown artifact: {0}")]
    UnknownArtifact(String),

}