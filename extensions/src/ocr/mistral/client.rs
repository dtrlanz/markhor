use std::path::Path;

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Response, StatusCode};
use tokio::fs::File;
use tracing::{debug, error, instrument, warn};
use url::Url;


use crate::ocr::mistral::error::MistralApiErrorResponse;
use crate::ocr::mistral::helpers::HttpValidationErrorResponse;

use super::error::{OcrError, SignedUrlError};
use super::helpers::{FileUploadError, FileUploadResponse, OcrRequest, OcrResponse, SignedUrlResponse}; // Added instrument for tracing

const MISTRAL_API_BASE: &str = "https://api.mistral.ai/v1";



// Assume a client structure like this exists
pub struct MistralClient {
    client: Client,
    api_key: String,
    // other fields like base_url if needed
}

impl MistralClient {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(), // Or reuse an existing client
            api_key,
        }
    }

    /// Processes a document using the Mistral OCR API.
    ///
    /// The document source must be a public URL or a pre-signed URL
    /// obtained from Mistral's file upload service (upload mechanism not implemented here).
    #[instrument(skip(self, request), fields(model = %request.model))] // Added tracing span
    pub async fn process_document(
        &self,
        request: &OcrRequest,
    ) -> Result<OcrResponse, OcrError> {
        let url = format!("{}/ocr", MISTRAL_API_BASE);
        debug!(target: "mistral_api::ocr", url = %url, "Sending OCR request");

        let response = self
            .client
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            .header(CONTENT_TYPE, "application/json")
            .json(request) // reqwest handles serialization here
            .send()
            .await;

        let response = match response {
            Ok(resp) => resp,
            Err(e) => {
                error!(target: "mistral_api::ocr", error = %e, "OCR request failed");
                // If the error is sensitive (e.g. contains URL with secrets), filter it
                return Err(OcrError::RequestFailed(e));
            }
        };

        Self::handle_response(response).await
    }

    /// Helper function to handle OCR success/error responses (REFINED)
    #[instrument(skip(response), fields(status = response.status().as_u16()))]
    async fn handle_response(response: Response) -> Result<OcrResponse, OcrError> { // Returns OcrError now
        let status = response.status();

        if status.is_success() {
            debug!(target: "mistral_api::ocr", "OCR request successful");
            // Attempt to deserialize the successful response
            match response.json::<OcrResponse>().await {
                Ok(ocr_response) => Ok(ocr_response),
                Err(e) => {
                     error!(target: "mistral_api::ocr", error = %e, "Failed to deserialize success response");
                     // This indicates a problem even on success status - treat as request error
                     Err(OcrError::RequestFailed(e)) // Or a more specific "UnexpectedResponseBody" error?
                }
            }
        } else {
            error!(target: "mistral_api::ocr", "OCR request returned error status");

            // --- START REFINEMENT ---
            // Explicitly check for 422 Unprocessable Entity
            if status == StatusCode::UNPROCESSABLE_ENTITY { // 422
                 match response.json::<HttpValidationErrorResponse>().await {
                    Ok(validation_error) => {
                        error!(target: "mistral_api::ocr", ?validation_error, "Parsed validation error details");
                         Err(OcrError::ValidationError {
                            status: status.as_u16(),
                            details: validation_error.detail,
                        })
                    }
                    Err(e) => {
                        // Status was 422, but body didn't match expected structure
                        warn!(target: "mistral_api::ocr", error = %e, "Status was 422, but failed to parse HttpValidationErrorResponse");
                         Err(OcrError::ErrorDeserializationFailed { status: status.as_u16(), source: e })
                         // Optionally, try reading body as text and returning generic ApiError
                    }
                }
            } else {
                // Handle other non-422 error statuses using the generic structure
                match response.json::<MistralApiErrorResponse>().await {
                     Ok(api_error) => {
                        error!(target: "mistral_api::ocr", ?api_error, "Parsed generic API error details");
                         Err(OcrError::ApiError {
                             status: status.as_u16(),
                             code: api_error.code,
                             message: api_error.message,
                         })
                     }
                     Err(e) => {
                        error!(target: "mistral_api::ocr", error = %e, "Failed to deserialize generic error response body");
                         Err(OcrError::ErrorDeserializationFailed { status: status.as_u16(), source: e })
                        // Optionally, try reading body as text for ApiError message
                        // let body_text = response.text().await.unwrap_or_else(|_| format!("API error status {}", status));
                        // Err(OcrError::ApiError { status: status.as_u16(), code: None, message: body_text })
                     }
                 }
            }
            // --- END REFINEMENT ---
        }
    }

    /// Uploads a file to be used by Mistral APIs (e.g., for OCR).
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the local file to upload.
    /// * `purpose` - The purpose of the file (e.g., "ocr", "fine-tune", "batch").
    ///
    /// # Returns
    ///
    /// A `Result` containing the `FileUploadResponse` on success, or a `FileUploadError` on failure.
    #[instrument(skip(self, file_path), fields(purpose = %purpose, path = %file_path.as_ref().display()))]
    pub async fn upload_file(
        &self,
        file_path: impl AsRef<Path>,
        purpose: &str,
    ) -> Result<FileUploadResponse, FileUploadError> {
        let file_path = file_path.as_ref();

        // Basic path validation
        if !file_path.is_file() {
            error!(target: "mistral_api::files", "Path is not a file or does not exist");
            return Err(FileUploadError::InvalidPath(file_path.to_path_buf()));
        }

        // Extract filename - required for the multipart part
        let filename = file_path
            .file_name()
            .and_then(|os_str| os_str.to_str())
            .map(|s| s.to_string())
            .ok_or_else(|| FileUploadError::FileNameError(file_path.to_path_buf()))?;

        debug!(target: "mistral_api::files", %filename, "Extracted filename");

        // Open the file asynchronously
        let file = File::open(file_path).await?; // Propagates std::io::Error via From

        // Create a stream from the file for reqwest
        // Use `with_length` hint if possible, otherwise reqwest chunks it.
        let file_size = file.metadata().await?.len();
        let stream = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
        let file_part = Part::stream_with_length(stream, file_size)
            .file_name(filename.clone());
            // Might need to explicitly set MIME type if not inferred correctly,
            // though reqwest often does a good job based on filename extension.
            //.mime_str("application/pdf")?;

        // Build the multipart form
        let form = Form::new()
            .text("purpose", purpose.to_string())
            .part("file", file_part);

        let url = format!("{}/files", MISTRAL_API_BASE);
        debug!(target: "mistral_api::files", url = %url, "Sending file upload request");

        // Send the request
        let response = self
            .client
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            // Content-Type is set automatically by .multipart()
            .multipart(form)
            .send()
            .await;

        let response = match response {
            Ok(resp) => resp,
            Err(e) => {
                error!(target: "mistral_api::files", error = %e, "File upload request failed");
                return Err(FileUploadError::RequestFailed(e));
            }
        };

        // Handle the response (success or various errors)
        Self::handle_file_upload_response(response).await
    }

    /// Helper function specific to handling file upload responses.
    #[instrument(skip(response), fields(status = response.status().as_u16()))]
    async fn handle_file_upload_response(response: Response) -> Result<FileUploadResponse, FileUploadError> {
        let status = response.status();

        if status.is_success() { // Assuming 2xx is success (e.g., 200 OK or 201 Created)
            debug!(target: "mistral_api::files", "File upload successful");
            match response.json::<FileUploadResponse>().await {
                Ok(upload_response) => Ok(upload_response),
                Err(e) => {
                    error!(target: "mistral_api::files", error = %e, "Failed to deserialize success response");
                    // If success status but bad body, treat as reqwest error
                    Err(FileUploadError::RequestFailed(e))
                }
            }
        } else {
            error!(target: "mistral_api::files", "File upload returned error status");

            // Special handling for 422 Validation Error
            if status == StatusCode::UNPROCESSABLE_ENTITY { // 422
                match response.json::<HttpValidationErrorResponse>().await {
                    Ok(validation_error) => {
                        error!(target: "mistral_api::files", ?validation_error, "Parsed validation error details");
                        Err(FileUploadError::ValidationError {
                            status: status.as_u16(),
                            details: validation_error.detail,
                        })
                    }
                    Err(e) => {
                        // Failed to parse the specific 422 structure, treat as generic error below
                        warn!(target: "mistral_api::files", error = %e, "Status was 422, but failed to parse HttpValidationErrorResponse");
                        // Fall through to generic error parsing attempt
                        Err(FileUploadError::ErrorDeserializationFailed { status: status.as_u16(), source: e })
                        // Alternatively, try to read body as text and put in generic ApiError message?
                    }
                }
            } else {
                // Handle other generic API errors
                match response.json::<MistralApiErrorResponse>().await {
                    Ok(api_error) => {
                        error!(target: "mistral_api::files", ?api_error, "Parsed generic API error details");
                        Err(FileUploadError::ApiError {
                            status: status.as_u16(),
                            code: api_error.code,
                            message: api_error.message,
                        })
                    }
                    Err(e) => {
                        error!(target: "mistral_api::files", error = %e, "Failed to deserialize generic error response body");
                        Err(FileUploadError::ErrorDeserializationFailed { status: status.as_u16(), source: e })
                        // Could also try reading body as text here for better error message
                        // let body_text = response.text().await.unwrap_or_else(|_| "Could not read error body".to_string());
                        // Err(FileUploadError::ApiError { status: status.as_u16(), code: None, message: body_text })
                    }
                }
            }
        }
    }

    /// Fetches a temporary signed URL for a previously uploaded file.
    ///
    /// This URL can typically be used as input for other API calls like OCR.
    ///
    /// # Arguments
    ///
    /// * `file_id` - The unique ID of the file obtained from the `upload_file` response.
    /// * `expiry_hours` - Optional duration (in hours) for which the URL should be valid.
    ///                    Consult Mistral documentation for default/min/max values.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `SignedUrlResponse` on success, or a `SignedUrlError` on failure.
    #[instrument(skip(self), fields(file_id = %file_id, expiry = ?expiry_hours))]
    pub async fn get_signed_url(
        &self,
        file_id: &str,
        expiry_hours: Option<u32>,
    ) -> Result<SignedUrlResponse, SignedUrlError> {
        // Construct the base URL using the url crate for safety
        let base_url_str = format!("{}/files/{}/url", MISTRAL_API_BASE, file_id);
        let mut url = Url::parse(&base_url_str)?; // Handles base URL construction errors

        // Add expiry query parameter if provided
        if let Some(hours) = expiry_hours {
            url.query_pairs_mut().append_pair("expiry", &hours.to_string());
        }

        debug!(target: "mistral_api::files::url", url = %url, "Requesting signed URL");

        // Build and send the GET request
        let response = self
            .client
            .get(url) // Use the constructed Url object
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            .header(ACCEPT, "application/json") // Specify we want JSON back
            .send()
            .await;

        let response = match response {
            Ok(resp) => resp,
            Err(e) => {
                error!(target: "mistral_api::files::url", error = %e, "Signed URL request failed");
                return Err(SignedUrlError::RequestFailed(e));
            }
        };

        // Handle the response
        Self::handle_signed_url_response(response, file_id).await // Pass file_id for better 404 errors
    }


    /// Helper function specific to handling signed URL responses.
    #[instrument(skip(response), fields(status = response.status().as_u16(), file_id = %file_id))]
    async fn handle_signed_url_response(
        response: Response,
        file_id: &str, // Receive file_id for context
    ) -> Result<SignedUrlResponse, SignedUrlError> {
        let status = response.status();

        if status.is_success() { // Should be 200 OK
            debug!(target: "mistral_api::files::url", "Signed URL request successful");
            match response.json::<SignedUrlResponse>().await {
                Ok(url_response) => Ok(url_response),
                Err(e) => {
                    error!(target: "mistral_api::files::url", error = %e, "Failed to deserialize success response");
                    Err(SignedUrlError::RequestFailed(e)) // Treat as request error
                }
            }
        } else {
            error!(target: "mistral_api::files::url", "Signed URL request returned error status");

            // --- Specific Error Handling ---
            match status {
                StatusCode::NOT_FOUND => { // Handle 404 specifically
                    // Try to parse generic error body for message, but prioritize NotFound type
                    let api_error_opt = response.json::<MistralApiErrorResponse>().await.ok();
                    let message = api_error_opt.map_or_else(
                        || format!("File with ID '{}' not found.", file_id), // Default message
                        |ae| ae.message // Use API message if available
                    );
                    error!(target: "mistral_api::files::url", %message, "File not found (404)");
                    Err(SignedUrlError::NotFound {
                        file_id: file_id.to_string(),
                        message,
                    })
                }
                StatusCode::UNPROCESSABLE_ENTITY => { // Handle potential 422 for bad 'expiry'
                    match response.json::<HttpValidationErrorResponse>().await {
                        Ok(validation_error) => {
                            error!(target: "mistral_api::files::url", ?validation_error, "Parsed validation error details");
                            Err(SignedUrlError::ValidationError {
                                status: status.as_u16(),
                                details: validation_error.detail,
                            })
                        }
                        Err(e) => {
                            warn!(target: "mistral_api::files::url", error = %e, "Status was 422, but failed to parse HttpValidationErrorResponse");
                            Err(SignedUrlError::ErrorDeserializationFailed { status: status.as_u16(), source: e })
                        }
                    }
                }
                _ => { // Handle all other errors using the generic path
                    match response.json::<MistralApiErrorResponse>().await {
                        Ok(api_error) => {
                            error!(target: "mistral_api::files::url", ?api_error, "Parsed generic API error details");
                            Err(SignedUrlError::ApiError {
                                status: status.as_u16(),
                                code: api_error.code,
                                message: api_error.message,
                            })
                        }
                        Err(e) => {
                            error!(target: "mistral_api::files::url", error = %e, "Failed to deserialize generic error response body");
                            Err(SignedUrlError::ErrorDeserializationFailed { status: status.as_u16(), source: e })
                        }
                    }
                }
            }
        }
    }    

}
