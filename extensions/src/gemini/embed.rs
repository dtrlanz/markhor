use async_trait::async_trait;
use markhor_core::{embedding::{Embedder, Embedding, EmbeddingError, EmbeddingUseCase}, extension::Functionality};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, instrument, trace, warn};
use url::Url; // For logging/tracing
use secrecy::ExposeSecret;

use crate::gemini::error::map_response_error;

use super::{error::GeminiError, shared::{GeminiConfig, SharedGeminiClient, EXTENSION_URI}};

/// Embedder implementation for Google Gemini models via the Generative Language API.
#[derive(Debug, Clone)] // Clone shares the SharedGeminiClient efficiently
pub struct GeminiEmbedder {
    shared_client: SharedGeminiClient,
    model_name: String,         // User-facing model name, e.g., "embedding-001"
    model_path_segment: String, // Path segment for API calls, e.g., "models/embedding-001"
    task_type: Option<String>,  // Store the configured task type string
    use_case: EmbeddingUseCase, // The mapped use case enum
    dimensions: Option<usize>,  // Known dimensions for the model
}

impl GeminiEmbedder {
    /// Creates a new Gemini Embedder with default settings.
    ///
    /// # Arguments
    /// * `api_key`: Your Google AI API key.
    /// * `model_name`: The name of the embedding model (e.g., "embedding-001").
    pub fn new(
        api_key: impl Into<String>,
        model_name: impl Into<String>,
    ) -> Result<Self, GeminiError> {
        Self::new_with_options(api_key, model_name, None, None, None)
    }

    /// Creates a new Gemini Embedder with custom options.
    ///
    /// # Arguments
    /// * `api_key`: Your Google AI API key.
    /// * `model_name`: The name of the embedding model (e.g., "embedding-001").
    /// * `task_type`: Optional task type (e.g., "RETRIEVAL_DOCUMENT").
    /// * `api_base_url`: Optional custom base URL override.
    /// * `client_override`: Optional custom `reqwest::Client` to use.
    pub fn new_with_options(
        api_key: impl Into<String>,
        model_name: impl Into<String>,
        task_type: Option<String>,
        api_base_url: Option<String>,
        client_override: Option<Client>,
    ) -> Result<Self, GeminiError> {
        let model_name = model_name.into();
        if model_name.is_empty() {
            return Err(GeminiError::InvalidConfiguration("Model name cannot be empty".to_string()));
        }

        let mut config = GeminiConfig::new(api_key)?;
        if let Some(base_url_str) = api_base_url {
            config = config.base_url(&base_url_str)?;
        }

        Self::from_config(config, model_name, task_type, client_override)
    }

    /// Creates a new Gemini Embedder from a pre-built configuration.
    #[instrument(name = "gemini_embedder_from_config", skip(config, client_override), fields(model_name=%model_name))]
    pub fn from_config(
        config: GeminiConfig,
        model_name: String, // Now required directly
        task_type: Option<String>,
        client_override: Option<Client>,
    ) -> Result<Self, GeminiError> {
        let shared_client = SharedGeminiClient::new(config, client_override)?;

        let model_path_segment = format!("models/{}", model_name);

        // Map task_type string to EmbeddingUseCase enum
        let use_case = map_task_type_to_use_case(task_type.as_deref());

        // Determine dimensions based on known models
        let dimensions = match model_name.as_str() {
            "embedding-001" => Some(768),
            // Add other known Gemini models here
            _ => {
                warn!(model = %model_name, "Unknown Gemini embedding model, dimensions not set.");
                None
            }
        };

        debug!(model=%model_name, task_type=?task_type, use_case=?use_case, dimensions=?dimensions, "GeminiEmbedder created.");

        Ok(Self {
            shared_client,
            model_name,
            model_path_segment,
            task_type,
            use_case,
            dimensions,
        })
    }

    // Helper to build the specific batchEmbedContents URL
    fn build_batch_embed_url(&self) -> Result<Url, GeminiError> {
        let path_segment = format!("{}:batchEmbedContents", self.model_path_segment);
        self.shared_client.build_url(&path_segment) // No extra query besides API key
    }    
}

impl Functionality for GeminiEmbedder {
    fn extension_uri(&self) -> &str {
        EXTENSION_URI
    }
    fn id(&self) -> &str { "gemini-embedder" }
    //fn description(&self) -> &str { "Client for Google Gemini Embedding API (generativelanguage.googleapis.com)" }
}


/// Helper function to map Gemini task type strings to the EmbeddingUseCase enum.
pub fn map_task_type_to_use_case(task_type: Option<&str>) -> EmbeddingUseCase {
    match task_type {
        Some("RETRIEVAL_QUERY") => EmbeddingUseCase::RetrievalQuery,
        Some("RETRIEVAL_DOCUMENT") => EmbeddingUseCase::RetrievalDocument,
        Some("SEMANTIC_SIMILARITY") | Some("SIMILARITY") => EmbeddingUseCase::Similarity,
        Some("CLASSIFICATION") => EmbeddingUseCase::Classification,
        Some("CLUSTERING") => EmbeddingUseCase::Clustering,
        Some("QUESTION_ANSWERING") => EmbeddingUseCase::QuestionAnswering,
        Some("FACT_VERIFICATION") => EmbeddingUseCase::FactVerification,
        Some(other) if other.starts_with("CODE_") => EmbeddingUseCase::CodeRetrievalQuery, // Group code tasks
        Some(other) => EmbeddingUseCase::Other(other.to_string()),
        None => EmbeddingUseCase::General, // Default if no task type is specified
    }
}

// The maximum batch size for embedding requests.
// Common limit for embedding-001 is 100. Make this configurable?
const BATCH_LIMIT: usize = 100;

#[async_trait]
impl Embedder for GeminiEmbedder {
    #[instrument(skip(self, texts), fields(model=%self.model_name, num_texts=texts.len()))]
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
        // Inner async block returning Result<..., GeminiError>
        async {
            // 1. Pre-flight Input Validation
            if texts.is_empty() {
                debug!("Input texts slice is empty, returning empty embeddings.");
                return Ok(vec![]);
            }

            // Check batch size limit
            if texts.len() > BATCH_LIMIT {
                error!(requested = texts.len(), limit = BATCH_LIMIT, "Batch size exceeds limit");
                return Err(GeminiError::BatchTooLarge {
                    limit: Some(BATCH_LIMIT),
                    actual: texts.len(),
                });
            }
            // Note: InputTooLong checks would typically happen before calling `embed`,
            // but if the API returns a specific error for it, we could parse it in the error handling section.

            // 2. Build URL
            let url = self.build_batch_embed_url()?; // Uses shared client internally
            debug!(%url, "Sending batch embed request to Gemini");

            // 3. Construct Request Body
            let requests: Vec<GeminiEmbedRequest> = texts
                .iter()
                .map(|text| GeminiEmbedRequest {
                    model: &self.model_path_segment, // Use the pre-formatted path segment
                    content: GeminiContent {
                        parts: vec![GeminiPart { text: text }],
                    },
                    task_type: self.task_type.as_deref(),
                })
                .collect();

            let request_body = GeminiBatchRequest { requests };

            // 4. Serialize Request Body
            let request_json = serde_json::to_string(&request_body)
                .map_err(|e| {
                    error!(error = %e, "Failed to serialize Gemini embed request body");
                    GeminiError::RequestSerialization(e)
                })?;
             trace!(body = %request_json, "Constructed Gemini embed request body JSON");

            // 5. Send Request (Add API Key Header - different from chat)
            // NOTE: The Gemini Embedding API documentation often shows using x-goog-api-key header.
            // Let's switch to using the header here, assuming the shared URL builder *doesn't* add the key.
            // We might need to adjust the shared URL builder or add a separate auth method.
            // --- ASSUMPTION: build_batch_embed_url *does not* add the key= query param ---
            // --- We'll add the header instead ---
            let response = self.shared_client.http_client()
                .post(url)
                .header("x-goog-api-key", self.shared_client.config().api_key.expose_secret()) // API Key in header
                .header("Content-Type", "application/json")
                .body(request_json)
                .send()
                .await
                .map_err(GeminiError::Network)?;

            // 6. Check Response Status
            if !response.status().is_success() {
                let status = response.status();
                error!(%status, "Gemini embed API returned error status");
                // 7. Map Error Response
                return Err(map_response_error(response).await);
                // TODO: Potentially inspect the mapped GeminiError::ApiError here
                //       to see if the message/detail indicates InputTooLong specifically.
                //       If so, could return EmbeddingError::InputTooLong instead.
            }

            // 8. Process Successful Response
            let status = response.status();
            debug!(%status, "Received successful response for embed request");
            let raw_body = response.text()
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to read successful response body for embed");
                    GeminiError::Network(e)
                })?;
            trace!(body = %raw_body, "Received Gemini embed response body");

            // 9. Parse JSON Response
            let response_data: GeminiBatchResponse = serde_json::from_str(&raw_body)
                .map_err(|e| {
                    error!(parse_error = %e, raw_body = %raw_body, "Failed to parse Gemini embed response JSON");
                    GeminiError::ResponseParsing {
                        context: "Parsing batch embed response".to_string(),
                        source: e,
                    }
                })?;

            // 10. Validate Response Consistency
            if response_data.embeddings.len() != texts.len() {
                let msg = format!(
                    "API returned {} embeddings, but expected {}",
                    response_data.embeddings.len(), texts.len()
                );
                error!(message = %msg, "Mismatch between input text count and received embeddings count");
                return Err(GeminiError::UnexpectedResponse(msg));
            }

            // 11. Convert to public Embedding struct
            debug!("Successfully parsed Gemini embed response, received {} embeddings.", response_data.embeddings.len());
            let embeddings_vec = response_data.embeddings
                .into_iter()
                .map(|e| Embedding::from(e.values)) // Assumes From<Vec<f32>> for Embedding exists
                .collect();

            Ok(embeddings_vec)

        }
        .await
        .map_err(|err| { err.into() })
    }

    fn dimensions(&self) -> Option<usize> {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn intended_use_case(&self) -> EmbeddingUseCase {
        self.use_case.clone()
    }

    fn max_batch_size_hint(&self) -> Option<usize> {
        Some(BATCH_LIMIT)
    }

    fn max_chunk_length_hint(&self) -> Option<usize> {
        Some(CHAR_LENGTH_HINT)
    }
}

// Gemini embedding-001 has a 2048 token limit.
// Use a conservative characters estimate (e.g., 4 chars/token -> 8192)
// Round down slightly.
const CHAR_LENGTH_HINT: usize = 8000;

// --- Gemini API Request Structures ---


#[derive(Serialize, Debug)]
pub struct GeminiBatchRequest<'a> {
    pub requests: Vec<GeminiEmbedRequest<'a>>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GeminiEmbedRequest<'a> {
    pub model: &'a str, // Full model path, e.g., "models/embedding-001"
    pub content: GeminiContent<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_type: Option<&'a str>, // e.g., "RETRIEVAL_DOCUMENT"
    // title field is omitted for simplicity, can be added if needed
}

#[derive(Serialize, Debug)]
pub struct GeminiContent<'a> {
    pub parts: Vec<GeminiPart<'a>>,
}

#[derive(Serialize, Debug)]
pub struct GeminiPart<'a> {
    pub text: &'a str,
}

// --- Gemini API Response Structures ---

#[derive(Deserialize, Debug)]
pub struct GeminiBatchResponse {
    pub embeddings: Vec<GeminiEmbeddingValue>,
}

#[derive(Deserialize, Debug)]
pub struct GeminiEmbeddingValue {
    pub values: Vec<f32>,
}

// --- Gemini API Error Structure (Common Format) ---
#[derive(Deserialize, Debug)]
pub struct GeminiApiErrorResponse {
    pub error: GeminiApiErrorDetail,
}

#[derive(Deserialize, Debug)]
pub struct GeminiApiErrorDetail {
    pub code: i32,
    pub message: String,
    pub status: String, // e.g., "INVALID_ARGUMENT"
}

// --- Configuration ---

/// Configuration options for the Gemini Embedder.
#[derive(Debug, Clone)]
pub struct GeminiEmbedderOptions {
    /// Optional task type to optimize embeddings for.
    /// Examples: "RETRIEVAL_QUERY", "RETRIEVAL_DOCUMENT", "SEMANTIC_SIMILARITY", etc.
    /// Refer to Google Gemini API documentation for valid values.
    /// If None, the model's default behavior is used.
    pub task_type: Option<String>,

    /// Optional custom reqwest::Client. If None, a default client is created.
    pub client: Option<reqwest::Client>,

    /// Optional custom API base URL. Defaults to the standard Gemini API endpoint.
    pub api_base_url: Option<String>,
}

impl Default for GeminiEmbedderOptions {
    fn default() -> Self {
        Self {
            task_type: None,
            client: None,
            api_base_url: None,
        }
    }
}