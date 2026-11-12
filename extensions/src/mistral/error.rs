use reqwest::StatusCode;

/// Errors returned by the Mistral client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request could not be sent or the response could not be read.
    #[error("request failed: {0}")]
    Network(#[source] reqwest::Error),

    /// The API responded with a non-success status.
    #[error("Mistral API error ({status}): {message}")]
    Api {
        status: StatusCode,
        /// The error message from the response body, or the raw body if it could not be parsed.
        message: String,
        /// The `type` field of the error body, if present (e.g. `invalid_request_error`).
        kind: Option<String>,
    },

    /// The response body did not have the expected shape.
    #[error("invalid response: {0}")]
    Decode(#[from] serde_json::Error),

    /// Reading a local file failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The file type is not one that Mistral OCR accepts (or could not be determined).
    #[error("unsupported file type: {0}")]
    UnsupportedFileType(String),
}

impl Error {
    /// Whether retrying the same request later may succeed (rate limiting or a transient server
    /// error, per Mistral's error documentation).
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Network(e) => e.is_timeout() || e.is_connect(),
            Error::Api { status, .. } => matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504),
            _ => false,
        }
    }
}
