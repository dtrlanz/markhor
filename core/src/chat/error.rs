use thiserror::Error;
use std::error::Error as StdError;


#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Network error: {0}")]
    Network(#[source] Box<dyn StdError + Send + Sync>),

    #[error("Authentication failed: {0}")]
    Authentication(String),

    #[error("API error: status={status}, message={message}")]
    Api { status: u16, message: String },

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Rate limit exceeded")]
    RateLimited,

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Content moderated: {0}")]
    ContentModerated(String),

    #[error("Response parsing error: {0}")]
    Parsing(#[source] Box<dyn StdError + Send + Sync>),

    #[error("Streaming error: {0}")]
    Streaming(#[source] Box<dyn StdError + Send + Sync>),

    #[error("Feature not supported by model or provider: {0}")]
    NotSupported(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("Provider-specific error: {0}")]
    Provider(#[source] Box<dyn StdError + Send + Sync>),

    #[error("Operation cancelled")]
    Cancelled,

    // Tool use
    #[error("Tool use error: {0}")]
    ToolUseError(String),

    #[error("Unknown error: {0}")]
    Unknown(#[source] Box<dyn StdError + Send + Sync>),
}
