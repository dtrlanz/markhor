use std::path::PathBuf;

use serde::Deserialize;

use super::helpers::ValidationErrorDetail;

/// Represents errors that can occur during an OCR request.
#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("Network or request error: {0}")]
    RequestFailed(#[from] reqwest::Error),

    // For specific 422 errors
    #[error("API Validation Error (422): {details:?}")]
    ValidationError {
        status: u16, // Should always be 422
        details: Vec<ValidationErrorDetail>, // Reusing struct from file uploads
    },

    // For other non-2xx errors
    #[error("API Error: Status={status}, Code={code:?}, Message='{message}'")]
    ApiError {
        status: u16,
        code: Option<String>,
        message: String,
    },

    // For cases where the error body *itself* can't be parsed
    #[error("Failed to deserialize API error response (Status={status}): {source}")]
    ErrorDeserializationFailed {
        status: u16,
        source: reqwest::Error,
    },

     #[error("Invalid input: {0}")]
     InvalidInput(String), // For client-side validation if needed
}


/// Structure to attempt deserializing API error responses.
/// Adjust fields based on actual error responses from Mistral.
#[derive(Debug, Deserialize)]
pub struct MistralApiErrorResponse {
    // Assuming fields like 'code' and 'message', might need adjustment
    // Based on Python client structure, it might be nested under 'detail' or similar
    // e.g., code: Option<String>, message: Option<String>, detail: Option<String>
    // Let's assume a simple structure for now:
    pub code: Option<String>,
    pub message: String,
    // type: Option<String>, // Sometimes APIs include an error type
    // param: Option<String>, // Sometimes they indicate the problematic parameter
}



#[derive(Debug, thiserror::Error)]
pub enum OcrOutputError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to decode Base64 image data for '{image_id}': {source}")]
    Base64Decode {
        image_id: String,
        #[source]
        source: base64::DecodeError,
    },

    #[error("Output directory path is invalid or points to a file: {0}")]
    InvalidOutputPath(PathBuf),
}


/// Represents errors that can occur when fetching a signed URL.
#[derive(Debug, thiserror::Error)]
pub enum SignedUrlError {
    #[error("Network or request error: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("Failed to construct request URL: {0}")]
    UrlConstructionError(#[from] url::ParseError), // Error from url crate

    // Specific error for 404 Not Found
    #[error("File not found (404): ID='{file_id}', Message='{message}'")]
    NotFound {
        file_id: String,
        message: String, // Capture message from API if available
    },

    // Reusing validation error structure for potential 422 on bad expiry
    #[error("API Validation Error (422): {details:?}")]
    ValidationError {
        status: u16,
        details: Vec<ValidationErrorDetail>,
    },

    // Generic API error for other statuses
    #[error("API Error: Status={status}, Code={code:?}, Message='{message}'")]
    ApiError {
        status: u16,
        code: Option<String>,
        message: String,
    },

    // Error for when the error body itself cannot be parsed
    #[error("Failed to deserialize API error response (Status={status}): {source}")]
    ErrorDeserializationFailed {
        status: u16,
        source: reqwest::Error,
    },
}