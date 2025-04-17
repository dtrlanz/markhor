use thiserror::Error;
use std::error::Error as StdError;

#[derive(Error, Debug)]
pub enum ChatError {
    /// Network error during API communication (e.g., connection refused, timeout, DNS resolution failure).
    #[error("Network error: {0}")]
    Network(#[source] Box<dyn StdError + Send + Sync>),

    /// Authentication failed (e.g., invalid API key, insufficient permissions).
    #[error("Authentication failed: {0}")]
    Authentication(String), // Context required

    /// Error reported by the API backend (e.g., bad request, server error).
    /// Check status for standard HTTP codes if available.
    #[error("API error: status={status:?}, message={message}")]
    Api {
        /// Optional HTTP status code from the API response.
        status: Option<u16>,
        /// Error message provided by the API or synthesized by the client.
        message: String, // Message required
        /// Optional underlying error (e.g., error parsing the error response).
        #[source]
        source: Option<Box<dyn StdError + Send + Sync>>,
    },

    /// The request payload was deemed invalid by the client *before* sending (e.g., missing required fields).
    #[error("Invalid request: {0}")]
    InvalidRequest(String), // Description required

    /// The API indicated a rate limit was exceeded.
    #[error("Rate limit exceeded")]
    RateLimited, // Specific type, often actionable (retry-after)

    /// The requested model is not available or not found.
    #[error("Model not found: {0}")]
    ModelNotFound(String), // Model name required

    /// The request or response was blocked due to content moderation policies.
    #[error("Content moderated: {0}")]
    ContentModerated(String), // Reason required

    /// Error parsing a *successful* response from the API.
    #[error("Response parsing error: {0}")]
    Parsing(#[source] Box<dyn StdError + Send + Sync>),

    /// Error specific to handling streaming responses.
    #[error("Streaming error: {0}")]
    Streaming(#[source] Box<dyn StdError + Send + Sync>),

    /// The requested feature or parameter is not supported by the model or provider.
    #[error("Feature not supported: {0}")]
    NotSupported(String), // Description required

    /// Error related to the configuration of the client or provider.
    #[error("Configuration error: {0}")]
    Configuration(String), // Description required

    /// Error related to the definition or execution of tools (function calling).
    #[error("Tool use error: {0}")]
    ToolUseError(String), // Description required

    /// The operation was cancelled (e.g., due to timeout or explicit request).
    #[error("Operation cancelled")]
    Cancelled,

    /// An error specific to the underlying provider/implementation that doesn't fit other categories.
    /// The source error should provide details.
    #[error("Provider-specific error: {0}")]
    Provider(#[source] Box<dyn StdError + Send + Sync>),
}