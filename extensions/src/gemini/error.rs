use reqwest::StatusCode;
use serde::Deserialize;
use std::error::Error as StdError;
use thiserror::Error;
use tracing::warn;

use markhor_core::{chat::error::ChatError, embedding::EmbeddingError};

// ============== Shared Gemini API Error Structures ==============

/// Represents the common error structure returned by the Gemini API.
#[derive(Deserialize, Debug, Clone)]
pub struct GeminiErrorResponse {
    pub error: GeminiErrorDetail,
}

/// Details of a Gemini API error.
#[derive(Deserialize, Debug, Clone)]
pub struct GeminiErrorDetail {
    /// HTTP status code associated with the error (might differ from response status).
    pub code: u16, // Usually matches HTTP status but good to capture
    /// Developer-facing error message.
    pub message: String,
    /// Status string (e.g., "INVALID_ARGUMENT", "UNAUTHENTICATED").
    pub status: String,
    // Potentially add 'details' field if needed later:
    // pub details: Option<serde_json::Value>,
}

// ============== Internal Gemini Client Error Enum ==============

/// Internal error type consolidating all possible failures within the Gemini client.
/// This type is intended to be converted into the public `ChatError` or `EmbeddingError`
/// at the trait implementation boundaries.
#[derive(Error, Debug)]
pub enum GeminiError {
    /// Error during network communication (sending request, reading response).
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error), // Use #[from] for easy conversion

    /// Error serializing the request body to JSON.
    #[error("Failed to serialize request body: {0}")]
    RequestSerialization(#[source] serde_json::Error),

    /// Error parsing a *successful* response body from the API.
    #[error("Failed to parse successful response body ({context}): {source}")]
    ResponseParsing {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    /// Error reported by the Gemini API (received non-success status code).
    #[error("Gemini API error: status={status}, message='{body_text}'")]
    ApiError {
        /// HTTP status code received from the API.
        status: StatusCode,
        /// Parsed error details from the response body, if available.
        detail: Option<GeminiErrorDetail>,
        /// Raw response body text.
        body_text: String,
    },

    /// Invalid configuration provided to the client.
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    /// Invalid input provided to an API method (e.g., empty text list, batch size exceeded).
    /// This is for validation *before* sending the request or based on API feedback clearly
    /// indicating bad input (like specific 400 errors).
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// The API returned an unexpected response format or data inconsistency.
    /// (e.g., embedding count mismatch, missing expected fields in success response).
    #[error("Unexpected response format or data: {0}")]
    UnexpectedResponse(String),

    /// Error specific to streaming operations (placeholder for future implementation).
    #[error("Streaming error: {0}")]
    Streaming(String),

    /// The number of input chunks exceeds the batch size limit.
    #[error("Input batch size too large (limit: {limit:?}, actual: {actual})")]
    BatchTooLarge {
        /// The maximum allowed batch size, if known.
        limit: Option<usize>,
        /// The actual batch size provided.
        actual: usize, // Actual size required
    },

    // Add other specific internal errors as needed
}

// ============== Shared Error Mapping Logic ==============

/// Shared helper function to process a `reqwest::Response` known to be an error
/// (i.e., status code is not successful) and convert it into a `GeminiError::ApiError`.
///
/// Attempts to read the response body and parse it as a `GeminiErrorResponse`.
/// If reading or parsing fails, it still returns a `GeminiError::ApiError`
/// with the raw body text and no parsed detail.
///
/// # Arguments
/// * `response`: The `reqwest::Response` object with a non-success status code.
///
/// # Returns
/// A `GeminiError` representing the API error. Returns `GeminiError::Network` if
/// reading the response body fails entirely.
pub(crate) async fn map_response_error(response: reqwest::Response) -> GeminiError {
    let status = response.status();
    debug_assert!(!status.is_success(), "map_response_error called with success status");

    // Try to read the body, capturing potential network error during read
    let body_text_result = response.text().await;

    match body_text_result {
        Ok(body_text) => {
            // Attempt to parse the known Gemini error structure
            match serde_json::from_str::<GeminiErrorResponse>(&body_text) {
                Ok(parsed_error) => {
                    // Successfully parsed the Gemini error structure
                    GeminiError::ApiError {
                        status,
                        detail: Some(parsed_error.error),
                        body_text,
                    }
                }
                Err(parse_err) => {
                    // Failed to parse, but we have the raw body text
                    warn!(
                        status = %status,
                        error = %parse_err,
                        body = %body_text,
                        "Failed to parse Gemini error response JSON, returning raw body."
                    );
                    GeminiError::ApiError {
                        status,
                        detail: None, // No parsed detail
                        body_text,
                    }
                }
            }
        }
        Err(e) => {
            // Failed even to read the error body text
            warn!(
                status = %status,
                error = %e,
                "Failed to read Gemini error response body text."
            );
            // Return as a network error, as reading the response failed
            GeminiError::Network(e)
        }
    }
}





// ============== From<GeminiError> for ChatError ==============

impl From<GeminiError> for ChatError {
    fn from(err: GeminiError) -> Self {
        match err {
            GeminiError::Network(source) => {
                // Check if the error is specifically a timeout
                if source.is_timeout() {
                    // Potentially map to Cancelled or keep as Network?
                    // Let's keep it Network for now, but add the source.
                    ChatError::Network(Box::new(source))
                } else {
                    ChatError::Network(Box::new(source))
                }
            }
            GeminiError::RequestSerialization(source) => {
                // This is an internal client error preparing the request.
                // Map to ProviderSpecific or InvalidRequest? InvalidRequest seems better fit.
                ChatError::InvalidRequest(format!("Failed to serialize request: {}", source))
                // Alternatively, if source preservation is desired:
                // ChatError::Provider(Box::new(err))
            }
            GeminiError::ResponseParsing { context, source } => {
                // Error parsing a *successful* response.
                ChatError::Parsing(Box::new(source)) // Keep the original parsing error
            }
            GeminiError::ApiError { status, detail, body_text } => {
                // Map based on HTTP status code, using detail message if available.
                let message = detail
                    .map(|d| format!("{} (Status: {}, Code: {})", d.message, d.status, d.code))
                    .unwrap_or_else(|| body_text.clone()); // Use clone here as body_text is needed below too

                match status {
                    // --- Specific Mappings ---
                    StatusCode::BAD_REQUEST => ChatError::InvalidRequest(message), // 400
                    StatusCode::UNAUTHORIZED => ChatError::Authentication(message), // 401
                    StatusCode::FORBIDDEN => ChatError::Authentication(message),    // 403 (Often permissions)
                    StatusCode::NOT_FOUND => ChatError::ModelNotFound(message),     // 404 (Likely model not found, but could be endpoint)
                    StatusCode::TOO_MANY_REQUESTS => ChatError::RateLimited,        // 429
                    StatusCode::INTERNAL_SERVER_ERROR // 500
                    | StatusCode::BAD_GATEWAY          // 502
                    | StatusCode::SERVICE_UNAVAILABLE // 503
                    | StatusCode::GATEWAY_TIMEOUT    // 504
                     => ChatError::Api {
                        status: Some(status.as_u16()),
                        message,
                        source: None, // Source is the API itself, captured in message
                     },

                    // --- Fallback ---
                    _ => {
                        // Use the generic Api variant for other client/server errors
                        ChatError::Api {
                           status: Some(status.as_u16()),
                           message,
                           source: None,
                        }
                    }
                }
            }
            GeminiError::InvalidConfiguration(msg) => ChatError::Configuration(msg),
            GeminiError::InvalidInput(msg) => ChatError::InvalidRequest(msg), // Input validation failed client-side
            GeminiError::UnexpectedResponse(msg) => {
                // This indicates a deviation from the expected API contract on success.
                // Map to Parsing or Provider? Provider seems more appropriate as it's an implementation detail.
                ChatError::Provider(msg.into()) // Convert String -> Box<dyn Error...>
            }
            GeminiError::Streaming(msg) => {
                // Map to ChatError::Streaming, preserving the message.
                // Since Streaming variant expects a source, wrap the message.
                ChatError::Streaming(msg.into())
            }
            GeminiError::BatchTooLarge { .. } => {
                // Batch size is not applicable to chat calls.
                // Map to Provider error.
                ChatError::Provider(format!("Unexpected batch size error during chat operation: {}", err).into())
            }
        }
    }
}


// ============== From<GeminiError> for EmbeddingError ==============

impl From<GeminiError> for EmbeddingError {
    fn from(err: GeminiError) -> Self {
        match err {
            GeminiError::Network(source) => EmbeddingError::Network(Box::new(source)),
            GeminiError::RequestSerialization(source) => {
                // Map to Provider or InvalidRequest
                // Preserve the source GeminiError here
                EmbeddingError::Provider(Box::new(GeminiError::RequestSerialization(source)))
            }
            GeminiError::ResponseParsing { context, source } => {
                EmbeddingError::Parsing(Box::new(source))
            }
            GeminiError::ApiError { status, detail, body_text } => {
                let message = detail
                    .map(|d| format!("{} (Status: {}, Code: {})", d.message, d.status, d.code))
                    .unwrap_or_else(|| body_text.clone());

                match status {
                    // --- Specific Mappings ---
                    StatusCode::BAD_REQUEST => {
                        // Could potentially be InputTooLong or BatchTooLarge if the API indicates it.
                        // Without specific error codes/messages from Gemini indicating this,
                        // map to InvalidRequest for now. Implementation might refine this
                        // by inspecting the `detail.status` or `message`.
                        // Example refinement (pseudo-code):
                        // if detail.as_ref().map_or(false, |d| d.message.contains("size limit")) {
                        //    EmbeddingError::BatchTooLarge { limit: None, actual: 0 /* How to get? Needs more context */ }
                        // } else {
                           EmbeddingError::InvalidRequest(message)
                        // }
                    }
                    StatusCode::UNAUTHORIZED => EmbeddingError::Authentication(message),
                    StatusCode::FORBIDDEN => EmbeddingError::Authentication(message),
                    StatusCode::NOT_FOUND => EmbeddingError::ModelNotFound(message),
                    StatusCode::TOO_MANY_REQUESTS => EmbeddingError::RateLimited,
                    StatusCode::INTERNAL_SERVER_ERROR
                    | StatusCode::BAD_GATEWAY
                    | StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::GATEWAY_TIMEOUT
                     => EmbeddingError::Api {
                        status: Some(status.as_u16()),
                        message,
                        source: None,
                     },

                    // --- Fallback ---
                    _ => EmbeddingError::Api {
                        status: Some(status.as_u16()),
                        message,
                        source: None,
                    }
                }
            }
            GeminiError::InvalidConfiguration(msg) => EmbeddingError::Configuration(msg),
            GeminiError::InvalidInput(msg) => {
                // This is where client-side validation errors go.
                // For now, map to generic InvalidRequest.
                EmbeddingError::InvalidRequest(msg)
            }
            GeminiError::UnexpectedResponse(msg) => {
                // e.g., embedding count mismatch. Map to Provider.
                EmbeddingError::Provider(msg.into())
            }
            GeminiError::Streaming(_) => {
                // Streaming is not applicable to the standard embedding call.
                // Map to Provider error.
                EmbeddingError::Provider(format!("Unexpected streaming error during embedding operation: {}", err).into())
            }
            GeminiError::BatchTooLarge { limit, actual } => {
                EmbeddingError::BatchTooLarge { limit , actual }
            }
        }
    }
}