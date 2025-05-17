use thiserror::Error;
use std::error::Error as StdError;

#[derive(Error, Debug)]
pub enum EmbeddingError {
    /// Network error during API communication (e.g., connection refused, timeout, DNS resolution failure).
    #[error("Network error: {0}")]
    Network(#[source] Box<dyn StdError + Send + Sync>), // Common

    /// Authentication failed (e.g., invalid API key, insufficient permissions).
    #[error("Authentication failed: {0}")]
    Authentication(String), // Common (Context required)

    /// Error reported by the API backend (e.g., bad request, server error).
    /// Check status for standard HTTP codes if available.
    #[error("API error: status={status:?}, message={message}")]
    Api { // Common (Standardized structure)
        /// Optional HTTP status code from the API response.
        status: Option<u16>,
        /// Error message provided by the API or synthesized by the client.
        message: String, // Message required
        /// Optional underlying error (e.g., error parsing the error response).
        #[source]
        source: Option<Box<dyn StdError + Send + Sync>>,
    },

    /// The request payload was deemed invalid by the client *before* sending (e.g., empty input list).
    #[error("Invalid request: {0}")]
    InvalidRequest(String), // Common (Description required)

    /// The API indicated a rate limit was exceeded.
    #[error("Rate limit exceeded")]
    RateLimited, // Common

    /// The requested embedding model is not available or not found.
    #[error("Model not found: {0}")]
    ModelNotFound(String), // Common (Model name required)

    /// Error parsing a *successful* response from the API.
    #[error("Response parsing error: {0}")]
    Parsing(#[source] Box<dyn StdError + Send + Sync>), // Common

    /// An input text chunk exceeds the model's length limit.
    #[error("Input text too long (limit: {limit:?}, unit: {unit}, index: {index:?})")]
    InputTooLong { // Specific
        /// The maximum allowed length, if known.
        limit: Option<usize>,
        /// The unit of the limit (e.g., "tokens", "characters").
        unit: String, // Unit context required
        /// Optional index of the offending text chunk in the input slice. `None` if not determined.
        index: Option<usize>,
    },

    /// The number of input chunks exceeds the batch size limit.
    #[error("Input batch size too large (limit: {limit:?}, actual: {actual})")]
    BatchTooLarge { // Specific
        /// The maximum allowed batch size, if known.
        limit: Option<usize>,
        /// The actual batch size provided.
        actual: usize, // Actual size required
    },

    /// Error related to the configuration of the client or provider.
    #[error("Configuration error: {0}")]
    Configuration(String), // Common (Description required)

    /// Error loading or initializing a local embedding model (if applicable).
    #[error("Error loading or running local model: {0}")]
    ModelLoadError(#[source] Box<dyn StdError + Send + Sync>), // Specific to local models

    /// The operation was cancelled (e.g., due to timeout or explicit request).
    #[error("Operation cancelled")]
    Cancelled, // Common

    /// An error specific to the underlying provider/implementation that doesn't fit other categories.
    /// The source error should provide details.
    #[error("Provider-specific error: {0}")]
    Provider(#[source] Box<dyn StdError + Send + Sync>), // Common (Catch-all)
}


// // Helper conversions (can add more as needed)

// // Conversion from reqwest::Error
// // Todo: Add feature flag for reqwest to avoid unnecessary dependency
// //#[cfg(feature = "reqwest")]
// impl From<reqwest::Error> for EmbeddingError {
//     fn from(err: reqwest::Error) -> Self {
//         let boxed_err = Box::new(err) as Box<dyn StdError + Send + Sync>;
//         // Try to classify based on reqwest error kind
//         let err_ref = boxed_err.downcast_ref::<reqwest::Error>().unwrap(); // Safe cast back

//         if err_ref.is_connect() || err_ref.is_timeout() {
//             EmbeddingError::Network(boxed_err)
//         } else if err_ref.is_status() {
//             let status = err_ref.status();
//              EmbeddingError::ApiError {
//                 status_code: status.map(|s| s.as_u16()),
//                 message: format!("HTTP status error: {}", status.unwrap_or_default()),
//                 source: Some(boxed_err), // Include the original reqwest::Error as source
//             }
//         } else if err_ref.is_request() || err_ref.is_body() || err_ref.is_decode() {
//             // These could be API errors (bad request format) or network issues,
//             // or internal serialization issues. ApiError or ImplementationSpecific might fit.
//              EmbeddingError::ApiError {
//                 status_code: None, // Status not necessarily available
//                 message: format!("API request/response handling error: {}", boxed_err),
//                 source: Some(boxed_err),
//             }
//         } else {
//             // Default to ImplementationSpecific or wrap directly
//              EmbeddingError::ImplementationSpecific(boxed_err)
//             // Or potentially: EmbeddingError::External(boxed_err)
//         }
//     }
// }

// // Conversion from serde_json::Error (often happens when parsing API responses)
// impl From<serde_json::Error> for EmbeddingError {
//      fn from(err: serde_json::Error) -> Self {
//         EmbeddingError::ApiError { // Often implies unexpected API response format
//             status_code: None,
//             message: format!("Failed to parse JSON response: {}", err),
//             source: Some(Box::new(err)),
//         }
//     }
// }

// // Conversion from std::io::Error (might occur with local models)
// impl From<std::io::Error> for EmbeddingError {
//     fn from(err: std::io::Error) -> Self {
//         // Could be ModelError (file not found) or ImplementationSpecific
//         EmbeddingError::ModelError(Box::new(err))
//     }
// }


