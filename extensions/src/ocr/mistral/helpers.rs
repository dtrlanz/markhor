use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument, warn};
use std::path::{Path, PathBuf};
use base64::{engine::general_purpose::STANDARD as Base64Standard, Engine};
use tokio::fs;

use super::error::OcrOutputError; // Only needed if we want arbitrary extra fields

// --- Request Structures ---

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OcrRequest {
    /// The model ID to use for the request. E.g., "mistral-ocr-latest".
    pub model: String, // Seems non-nullable based on examples

    /// The document input source.
    pub document: DocumentInput,

    /// Optional identifier for the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Optional list of 0-based page indices to process.
    /// If None or empty, all pages are processed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<u32>>, // Using u32 for page indices

    /// Whether to include base64 encoded images in the response. Defaults to false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_image_base64: Option<bool>,

    /// Optional maximum number of images to extract per page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_limit: Option<u32>,

    /// Optional minimum height and width (in pixels) of images to extract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_min_size: Option<u32>,
}

/// Represents the input document, discriminated by the 'type' field.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentInput {
    /// Process a document available at a public or pre-signed URL.
    DocumentUrl { document_url: String },
    /// Process an image available at a public or pre-signed URL.
    ImageUrl { image_url: String },
    // Add other potential types here later if discovered (e.g., raw bytes, file IDs)
}

// --- Response Structures ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OcrResponse {
    /// List of processed pages and their content.
    pub pages: Vec<PageDetail>,
    /// The specific model version used for processing.
    pub model: String,
    /// Information about the usage for this request.
    pub usage_info: UsageInfo,
    // #[serde(flatten)]
    // extra: HashMap<String, serde_json::Value>, // Optionally capture unknown fields
}

impl OcrResponse {
    /// Saves the OCR response content to the specified directory.
    ///
    /// Creates the directory if it doesn't exist.
    /// Writes the combined Markdown content to `output_dir/output.md`.
    /// Saves any extracted images to `output_dir/images/`.
    ///
    /// # Arguments
    ///
    /// * `output_dir`: The path to the directory where files should be saved.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success, or an `OcrOutputError` on failure.
    #[instrument(skip(self), fields(output_dir = %output_dir.as_ref().display()))]
    pub async fn save_to_files(
        &self,
        output_dir: impl AsRef<Path>,
    ) -> Result<(), OcrOutputError> {
        let output_dir = output_dir.as_ref();

        // Ensure output path is a directory or creatable, not a file
        if output_dir.is_file() {
            return Err(OcrOutputError::InvalidOutputPath(output_dir.to_path_buf()));
        }

        // Create base directory
        fs::create_dir_all(&output_dir).await?;
        debug!(target: "mistral_ocr::output", path = %output_dir.display(), "Ensured output directory exists");

        // 1. Combine and write Markdown
        let md_path = output_dir.join("output.md");
        let mut combined_markdown = String::new();
        // Sort pages by index just in case they aren't ordered (though they likely are)
        let mut sorted_pages = self.pages.clone(); // Clone to sort if needed, or iterate directly if order is guaranteed
        sorted_pages.sort_by_key(|p| p.index);

        for (i, page) in sorted_pages.iter().enumerate() {
            if i > 0 {
                combined_markdown.push_str("\n\n---\n\n"); // Add a separator between pages
            }
            // Optional: Add a page break
            // Todo: Requires more thought to avoid Markdown conflicts (e.g., with tables)
            //combined_markdown.push_str(&format!("<span id='page-{}' class='page-break'> {} | {} </span>", page.index + 1, page.index + 1, page.index + 1));

            combined_markdown.push_str(&page.markdown);
        }

        fs::write(&md_path, combined_markdown).await?;
        debug!(target: "mistral_ocr::output", path = %md_path.display(), "Wrote combined markdown");

        // 2. Decode and write images (with Data URI handling)
        let mut images_dir = None;

        let mut image_count = 0;
        for page in &sorted_pages {
            for image in &page.images {
                if !image.image_base64.is_empty() {
                    // Only create images subdirectory if we have images to save
                    if images_dir.is_none() {
                        // Create images subdirectory if it doesn't exist
                        let images_dir_path = output_dir.join("images");
                        fs::create_dir_all(&images_dir_path).await?;
                        debug!(target: "mistral_ocr::output", path = %images_dir_path.display(), "Ensured images directory exists");
                        images_dir = Some(images_dir_path);
                    }

                    // ---- START: Data URI Handling ----
                    let base64_data_to_decode = if image.image_base64.starts_with("data:") {
                         // Find the comma separating the prefix from the data
                         if let Some(comma_index) = image.image_base64.find(',') {
                             let data_part = &image.image_base64[comma_index + 1..];
                             debug!(target: "mistral_ocr::output", image_id = %image.id, "Detected and stripped Data URI prefix");
                             data_part
                         } else {
                            // Found "data:" but no comma? This is weird. Log and try decoding the whole thing anyway? Or error?
                            // Let's warn and proceed cautiously. The decode will likely fail.
                            warn!(target: "mistral_ocr::output", image_id = %image.id, "Found 'data:' prefix but no comma, attempting decode anyway");
                            &image.image_base64
                         }
                    } else {
                        // No "data:" prefix, assume it's raw base64
                        &image.image_base64
                    };
                    // ---- END: Data URI Handling ----


                    // Decode the (potentially stripped) base64 data
                    let image_data = Base64Standard.decode(base64_data_to_decode).map_err(|e| {
                        error!(target: "mistral_ocr::output", image_id = %image.id, error = %e, "Base64 decoding failed");
                        OcrOutputError::Base64Decode{ image_id: image.id.clone(), source: e }
                    })?;

                    // Use the image ID as the filename
                    let image_path = images_dir.as_ref().unwrap().join(&image.id);
                    fs::write(&image_path, image_data).await?;
                    image_count += 1;
                    debug!(target: "mistral_ocr::output", path = %image_path.display(), "Wrote image");
                }
            }
        }

        info!(target: "mistral_ocr::output", markdown_path = %md_path.display(), images_saved = image_count, "OCR output saved successfully");
        Ok(())
    }
}


#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PageDetail {
    /// 0-based index of the page.
    /// NOTE: The example showed 1-based, but Python spec comment said 0-based.
    /// Assuming 0-based here as it's more conventional in programming. Verify with testing.
    pub index: u32,
    /// Extracted content of the page in Markdown format.
    pub markdown: String,
    /// List of images extracted from this page.
    /// Only populated if `include_image_base64` was true in the request.
    pub images: Vec<ImageDetail>,
    /// Dimensions of the page.
    pub dimensions: PageDimensions,
    // #[serde(flatten)]
    // extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ImageDetail {
    /// Identifier for the image within the document context.
    pub id: String,
    /// X coordinate of the top-left corner.
    pub top_left_x: u32,
    /// Y coordinate of the top-left corner.
    pub top_left_y: u32,
    /// X coordinate of the bottom-right corner.
    pub bottom_right_x: u32,
    /// Y coordinate of the bottom-right corner.
    pub bottom_right_y: u32,
    /// Base64 encoded representation of the image.
    pub image_base64: String, // base64 crate can decode this if needed
    // #[serde(flatten)]
    // extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PageDimensions {
    /// Dots Per Inch (resolution).
    pub dpi: u32,
    /// Height in pixels.
    pub height: u32,
    /// Width in pixels.
    pub width: u32,
    // #[serde(flatten)]
    // extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UsageInfo {
    /// Number of pages processed in the request.
    pub pages_processed: u32,
    /// Size of the document in bytes (if available/applicable).
    pub doc_size_bytes: Option<u64>, // u64 for size, optional as it was null
                                     // #[serde(flatten)]
                                     // extra: HashMap<String, serde_json::Value>,
}



/// Represents the successful response from the file upload endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileUploadResponse {
    pub id: String,
    pub object: String, // Likely "file"
    pub bytes: u64,
    pub created_at: u64, // Unix timestamp
    pub filename: String,
    pub purpose: String,
    pub sample_type: Option<String>, // It might not always be present or determined
    pub num_lines: Option<u64>, // Nullable field
    pub source: String, // e.g., "upload"
}


/// Represents a specific validation error detail (part of the 422 response).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ValidationErrorDetail {
    /// Location of the error (e.g., ["body", "file"]). Using Value for flexibility.
    pub loc: Vec<serde_json::Value>,
    /// Error message description.
    pub msg: String,
    /// Type of error (e.g., "value_error").
    #[serde(rename = "type")] // Handle keyword 'type'
    pub error_type: String,
}


/// Represents the structure of a 422 Unprocessable Entity error response.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpValidationErrorResponse {
    pub detail: Vec<ValidationErrorDetail>,
}


/// Represents errors that can occur during file upload.
#[derive(Debug, thiserror::Error)]
pub enum FileUploadError {
    #[error("Invalid file path: {0}")]
    InvalidPath(PathBuf),

    #[error("Failed to access or read file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Network or request error: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("API Validation Error (422): {details:?}")]
    ValidationError {
        status: u16, // Should always be 422
        details: Vec<ValidationErrorDetail>,
    },

    #[error("API Error: Status={status}, Code={code:?}, Message='{message}'")]
    ApiError {
        status: u16,
        code: Option<String>,
        message: String,
    },

    #[error("Failed to deserialize API error response (Status={status}): {source}")]
    ErrorDeserializationFailed {
        status: u16,
        source: reqwest::Error,
    },
     #[error("Failed to extract filename from path: {0}")]
     FileNameError(PathBuf),
}


/// Represents the successful response containing the signed URL.
#[derive(Debug, Clone, Deserialize)]
pub struct SignedUrlResponse {
    pub url: String, // The actual field name is "url"
}

