use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use markhor_core::convert::{ConversionError, Converter};
use markhor_core::extension::{Extension, Functionality};
use mime::Mime;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Response, StatusCode};
use tokio::fs::{self, File};
use tracing::{debug, error, info, instrument, warn};
use url::Url;
use base64::{engine::general_purpose::STANDARD as Base64Standard, Engine as _}; // For saving images



use crate::ocr::mistral::error::MistralApiErrorResponse;
use crate::ocr::mistral::helpers::{DocumentInput, HttpValidationErrorResponse};

use super::converter::MistralOcr;
use super::error::{FileUploadError, OcrError, OcrToFileError, SignedUrlError};
use super::helpers::{FileUploadResponse, OcrRequest, OcrResponse, SignedUrlResponse}; // Added instrument for tracing

const MISTRAL_API_BASE: &str = "https://api.mistral.ai/v1";



// Assume a client structure like this exists
pub(crate) struct MistralClientInner {
    client: Client,
    api_key: String,
    // other fields like base_url if needed
}

impl MistralClientInner {
    /// Processes a document using the Mistral OCR API.
    ///
    /// The document source must be a public URL or a pre-signed URL
    /// obtained from Mistral's file upload service.
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

    /// Performs OCR on a local source file (PDF or Image) and saves the result.
    ///
    /// Saves the extracted Markdown content to `target_md_path`.
    /// Saves any extracted images into an `images` subdirectory within the
    /// parent directory of `target_md_path`.
    ///
    /// # Arguments
    ///
    /// * `source_path` - Path to the local source file (e.g., "input/report.pdf", "input/receipt.png").
    /// * `target_md_path` - Path where the output Markdown file should be saved (e.g., "output/report.md").
    ///                      Must end with ".md".
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success, or an `OcrToFileError` on failure.
    #[instrument(skip(self, source_path, target_md_path), fields(
        source = %source_path.as_ref().display(),
        target = %target_md_path.as_ref().display()
    ))]
    pub async fn ocr_file_to_markdown(
        &self,
        source_path: impl AsRef<Path>,
        target_md_path: impl AsRef<Path>,
    ) -> Result<(), OcrToFileError> {
        let source_path = source_path.as_ref();
        let target_md_path = target_md_path.as_ref();

        // --- Input Validation ---
        if !source_path.is_file() {
            return Err(OcrToFileError::InvalidSourcePath(format!(
                "Source path does not exist or is not a file: {}",
                source_path.display()
            )));
        }
        if !target_md_path.to_string_lossy().ends_with(".md") {
            return Err(OcrToFileError::InvalidTargetPath(
                "Target path must end with .md".to_string()
            ));
        }
        let output_base_dir = target_md_path.parent().ok_or_else(|| OcrToFileError::NoParentDirectory(target_md_path.to_path_buf()))?;


        // --- Determine Input Type from Extension ---
        let extension = source_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        let document_input_type = match extension.as_str() {
            "pdf" => "document",
            "png" | "jpg" | "jpeg" | "webp" | "bmp" // Add other supported image types if known
            => "image",
            _ => {
                warn!(target: "mistral_api::convenience", "Unknown source extension '{}', attempting as 'document'", extension);
                "document" // Default to document, or return UnsupportedFileType error
                // return Err(OcrToFileError::UnsupportedFileType(extension));
            }
        };
        debug!(target: "mistral_api::convenience", %document_input_type, "Determined input type");


        // --- Step 1: Upload File ---
        debug!(target: "mistral_api::convenience", "Step 1: Uploading source file...");
        let upload_response = self.upload_file(source_path, "ocr").await?; // Propagates FileUploadError via From
        let file_id = upload_response.id;
        debug!(target: "mistral_api::convenience", %file_id, "Upload successful");


        // --- Step 2: Get Signed URL ---
        debug!(target: "mistral_api::convenience", "Step 2: Getting signed URL...");
        let expiry_hours = Some(1u32); // 1 hour expiry seems reasonable
        let signed_url_response = self.get_signed_url(&file_id, expiry_hours).await?; // Propagates SignedUrlError via From
        let usable_url = signed_url_response.url;
        debug!(target: "mistral_api::convenience", "Signed URL obtained");


        // --- Step 3: Process Document/Image (OCR) ---
        debug!(target: "mistral_api::convenience", "Step 3: Calling OCR endpoint...");
        let document_input = match document_input_type {
            "document" => DocumentInput::DocumentUrl { document_url: usable_url },
            "image" => DocumentInput::ImageUrl { image_url: usable_url },
            _ => unreachable!(), // Should have been handled above
        };

        let ocr_request = OcrRequest {
            model: "mistral-ocr-latest".to_string(),
            document: document_input,
            include_image_base64: Some(true), // Need images for saving
            // Reset page/image limits if it's an image - API might ignore anyway
            pages: if document_input_type == "image" { None } else { None }, // Or keep None always? Test API behavior.
            image_limit: if document_input_type == "image" { None } else { None },
            image_min_size: if document_input_type == "image" { None } else { None },
            id: None,
        };

        let ocr_response = self.process_document(&ocr_request).await?; // Propagates OcrError via From
        debug!(target: "mistral_api::convenience", "OCR processing successful");


        // --- Step 4: Save Output (Custom Logic) ---
        debug!(target: "mistral_api::convenience", "Step 4: Saving results...");

        // Ensure the base output directory exists (parent of the target .md file)
        fs::create_dir_all(output_base_dir)
            .await
            .map_err(OcrToFileError::SaveIo)?;
        debug!(target: "mistral_api::convenience", path = %output_base_dir.display(), "Ensured output base directory exists");


        // 4a. Combine and write Markdown to the target path
        let mut combined_markdown = String::new();
        let mut sorted_pages = ocr_response.pages; // Use Vec directly
        sorted_pages.sort_by_key(|p| p.index); // Sort just in case

        for (i, page) in sorted_pages.iter().enumerate() {
            if i > 0 {
                combined_markdown.push_str("\n\n---\n\n"); // Page separator
            }
            combined_markdown.push_str(&page.markdown);
        }

        fs::write(target_md_path, combined_markdown)
            .await
            .map_err(OcrToFileError::SaveIo)?;
        debug!(target: "mistral_api::convenience", path = %target_md_path.display(), "Wrote Markdown file");


        // 4b. Decode and write images to parent/images/ directory
        let images_dir = output_base_dir.join("images");
        let mut images_found = false; // Track if we need to create the dir

        for page in &sorted_pages {
            for image in &page.images {
                if !image.image_base64.is_empty() {
                    // Create images dir only if we find the first image
                    if !images_found {
                        fs::create_dir_all(&images_dir)
                            .await
                            .map_err(OcrToFileError::SaveIo)?;
                        debug!(target: "mistral_api::convenience", path = %images_dir.display(), "Created images directory");
                        images_found = true;
                    }

                    // Strip data URI prefix (reuse logic from previous save function)
                    let base64_data_to_decode = if image.image_base64.starts_with("data:") {
                        image.image_base64.find(',').map_or_else(
                            || {
                                warn!(target: "mistral_api::convenience", image_id = %image.id, "Found 'data:' prefix but no comma");
                                image.image_base64.as_str()
                            },
                            |comma_index| &image.image_base64[comma_index + 1..],
                        )
                    } else {
                        &image.image_base64
                    };

                    // Decode
                    let image_data = Base64Standard.decode(base64_data_to_decode).map_err(|e| {
                        error!(target: "mistral_api::convenience", image_id = %image.id, error = %e, "Base64 decoding failed during save");
                        OcrToFileError::SaveBase64 { image_id: image.id.clone(), source: e }
                    })?;

                    // Write image file
                    let image_path = images_dir.join(&image.id); // Use image ID as filename
                    fs::write(&image_path, image_data)
                        .await
                        .map_err(OcrToFileError::SaveIo)?;
                    debug!(target: "mistral_api::convenience", path = %image_path.display(), "Wrote image");
                }
            }
        }

        if !images_found {
            debug!(target: "mistral_api::convenience", "No images found in OCR response to save.");
        }

        info!(target: "mistral_api::convenience", "OCR conversion to file completed successfully.");
        Ok(())
    }


}


pub struct MistralClient {
    inner: Arc<MistralClientInner>,
}

impl MistralClient {
    pub fn new(api_key: String) -> Self {
        let inner = Arc::new(MistralClientInner {
            client: Client::new(), // Or reuse an existing client
            api_key,
        });
        Self { inner }
    }

    pub fn process_document(
        &self,
        request: &OcrRequest,
    ) -> impl std::future::Future<Output = Result<OcrResponse, OcrError>> {
        self.inner.process_document(request)
    }

    pub fn upload_file(
        &self,
        file_path: impl AsRef<Path>,
        purpose: &str,
    ) -> impl std::future::Future<Output = Result<FileUploadResponse, FileUploadError>> {
        self.inner.upload_file(file_path, purpose)
    }

    pub fn get_signed_url(
        &self,
        file_id: &str,
        expiry_hours: Option<u32>,
    ) -> impl std::future::Future<Output = Result<SignedUrlResponse, SignedUrlError>> {
        self.inner.get_signed_url(file_id, expiry_hours)
    }

    pub fn ocr_file_to_markdown(
        &self,
        source_path: impl AsRef<Path>,
        target_md_path: impl AsRef<Path>,
    ) -> impl std::future::Future<Output = Result<(), OcrToFileError>> {
        self.inner.ocr_file_to_markdown(source_path, target_md_path)
    }
}

pub(crate) const URI: &str = "(mistral ocr extension uri)";
pub(crate) const NAME: &str = "Mistral Client";
pub(crate) const DESCRIPTION: &str = "Client for Mistral API (only OCR implemented so far)";

impl Extension for MistralClient {
    fn uri(&self) -> &str {
        URI
    }

    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn converter(&self) -> Option<Box<dyn Converter>> {
        Some(Box::new(MistralOcr(Arc::clone(&self.inner))))
    }
}