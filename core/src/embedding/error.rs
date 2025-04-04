use std::error::Error as StdError;
use thiserror::Error; // Using thiserror

// Define the custom error enum using thiserror
#[derive(Error, Debug)]
pub enum EmbeddingError {
    /// Error related to network communication (connection, timeouts, DNS).
    #[error("Network error: {0}")]
    Network(#[source] Box<dyn StdError + Send + Sync>), // Keep underlying error boxed

    /// Error reported by the remote API (invalid API key, rate limits, bad request, server errors).
    #[error("API error (Status: {status_code:?}): {message}")]
    ApiError {
        status_code: Option<u16>,
        message: String,
        #[source]
        source: Option<Box<dyn StdError + Send + Sync>>, // Optional underlying source (e.g., JSON parse error on error response)
    },

    /// Error specific to loading or running a local model (file not found, memory issues, format error).
    #[error("Model error: {0}")]
    ModelError(#[source] Box<dyn StdError + Send + Sync>),

    /// Error indicating an input text chunk exceeds the model's length limit.
    #[error("Input text at index {index:?} exceeds the maximum length limit ({limit:?} {unit})")]
    InputTooLong {
        /// The maximum allowed length (if known).
        limit: Option<usize>,
        /// The unit of the limit (e.g., "tokens", "characters"). Implementations should strive for clarity.
        unit: String,
        /// Optional index of the offending text chunk in the input slice.
        index: Option<usize>,
    },

    /// Error indicating the number of input chunks exceeds the batch size limit.
    #[error("Input batch size ({actual}) exceeds the maximum limit ({limit:?})")]
    BatchTooLarge {
         /// The maximum allowed batch size (if known).
        limit: Option<usize>,
         /// The actual batch size provided.
        actual: usize,
    },

    /// Error indicating generally invalid input (e.g., empty input slice, empty strings where not allowed).
    /// Use InputTooLong or BatchTooLarge for size limit errors.
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Error related to the configuration of the Embedder implementation.
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// A catch-all for errors specific to an implementation that don't fit other categories.
    #[error("Implementation-specific error: {0}")]
    ImplementationSpecific(#[source] Box<dyn StdError + Send + Sync>),

    /// Custom variant to wrap external errors when converting via From,
    /// ensuring the source is captured correctly by thiserror.
    /// Often used in From implementations for external errors like reqwest::Error or io::Error
    /// when they don't clearly map to Network, ApiError, or ModelError directly.
    #[error(transparent)]
    External(#[from] Box<dyn StdError + Send + Sync + 'static>),
}


// Helper conversions (can add more as needed)

// Conversion from reqwest::Error
// Todo: Add feature flag for reqwest to avoid unnecessary dependency
//#[cfg(feature = "reqwest")]
impl From<reqwest::Error> for EmbeddingError {
    fn from(err: reqwest::Error) -> Self {
        let boxed_err = Box::new(err) as Box<dyn StdError + Send + Sync>;
        // Try to classify based on reqwest error kind
        let err_ref = boxed_err.downcast_ref::<reqwest::Error>().unwrap(); // Safe cast back

        if err_ref.is_connect() || err_ref.is_timeout() {
            EmbeddingError::Network(boxed_err)
        } else if err_ref.is_status() {
            let status = err_ref.status();
             EmbeddingError::ApiError {
                status_code: status.map(|s| s.as_u16()),
                message: format!("HTTP status error: {}", status.unwrap_or_default()),
                source: Some(boxed_err), // Include the original reqwest::Error as source
            }
        } else if err_ref.is_request() || err_ref.is_body() || err_ref.is_decode() {
            // These could be API errors (bad request format) or network issues,
            // or internal serialization issues. ApiError or ImplementationSpecific might fit.
             EmbeddingError::ApiError {
                status_code: None, // Status not necessarily available
                message: format!("API request/response handling error: {}", boxed_err),
                source: Some(boxed_err),
            }
        } else {
            // Default to ImplementationSpecific or wrap directly
             EmbeddingError::ImplementationSpecific(boxed_err)
            // Or potentially: EmbeddingError::External(boxed_err)
        }
    }
}

// Conversion from serde_json::Error (often happens when parsing API responses)
impl From<serde_json::Error> for EmbeddingError {
     fn from(err: serde_json::Error) -> Self {
        EmbeddingError::ApiError { // Often implies unexpected API response format
            status_code: None,
            message: format!("Failed to parse JSON response: {}", err),
            source: Some(Box::new(err)),
        }
    }
}

// Conversion from std::io::Error (might occur with local models)
impl From<std::io::Error> for EmbeddingError {
    fn from(err: std::io::Error) -> Self {
        // Could be ModelError (file not found) or ImplementationSpecific
        EmbeddingError::ModelError(Box::new(err))
    }
}