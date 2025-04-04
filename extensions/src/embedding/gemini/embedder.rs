use async_trait::async_trait;
use markhor_core::embedding::{Embedder, EmbeddingError, EmbeddingUseCase, Embeddings};
use reqwest::Client;
use tracing::{debug, instrument, warn, error}; // For logging/tracing

use crate::embedding::gemini::helpers::{GeminiApiErrorResponse, GeminiBatchRequest, GeminiBatchResponse, GeminiContent, GeminiEmbedRequest, GeminiPart};

use super::helpers::GeminiEmbedderOptions;

/// Embedder implementation for Google Gemini models via the Generative Language API.
#[derive(Debug, Clone)] // Clone is possible because reqwest::Client is internally Arc-ed
pub struct GeminiEmbedder {
    client: Client,
    api_key: String,
    model_name: String, // User-facing model name, e.g., "embedding-001"
    model_id: String,   // Full model path for API calls, e.g., "models/embedding-001"
    task_type: Option<String>, // Store the configured task type string
    use_case: EmbeddingUseCase, // The mapped use case enum
    dimensions: Option<usize>, // Known dimensions for the model
    api_endpoint: String, // Pre-computed API endpoint URL
}

impl GeminiEmbedder {
    /// Creates a new Gemini Embedder.
    ///
    /// # Arguments
    ///
    /// * `api_key`: Your Google AI API key.
    /// * `model_name`: The name of the embedding model (e.g., "embedding-001").
    /// * `options`: Configuration options like task type or a custom client.
    ///
    /// # Errors
    ///
    /// Returns `EmbeddingError::Configuration` if the API key or model name is empty,
    /// or if there's an issue creating the default reqwest client.
    pub fn new(
        api_key: impl Into<String>,
        model_name: impl Into<String>,
        options: GeminiEmbedderOptions,
    ) -> Result<Self, EmbeddingError> {
        let api_key = api_key.into();
        let model_name = model_name.into();

        if api_key.is_empty() {
            return Err(EmbeddingError::Configuration("API key cannot be empty".to_string()));
        }
        if model_name.is_empty() {
            return Err(EmbeddingError::Configuration("Model name cannot be empty".to_string()));
        }

        let client = match options.client {
            Some(existing_client) => Ok(existing_client), // If a client was provided, wrap it in Ok
            None => {
                // If no client was provided, try to build one.
                // The closure now returns the Result directly.
                Client::builder()
                    .build()
                    .map_err(|e| EmbeddingError::Configuration(format!("Failed to create HTTP client: {}", e)))
            }
        }?;
        
        let model_id = format!("models/{}", model_name);
        let base_url = options.api_base_url.unwrap_or_else(||
            "https://generativelanguage.googleapis.com".to_string()
        );
        // Ensure base_url doesn't end with a slash for clean joining
        let base_url = base_url.trim_end_matches('/');
        let api_endpoint = format!("{}/v1beta/{}:batchEmbedContents", base_url, model_id);

        // Map task_type string to EmbeddingUseCase enum
        let use_case = map_task_type_to_use_case(options.task_type.as_deref());

        // Determine dimensions based on known models (can be expanded)
        let dimensions = match model_name.as_str() {
            "embedding-001" => Some(768),
            // Add other known Gemini models here
            _ => {
                warn!(model = %model_name, "Unknown Gemini model, dimensions not set.");
                None
            }
        };

        Ok(Self {
            client,
            api_key,
            model_name,
            model_id,
            task_type: options.task_type,
            use_case,
            dimensions,
            api_endpoint,
        })
    }
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
    async fn embed(&self, texts: &[&str]) -> Result<Embeddings, EmbeddingError> {
        if texts.is_empty() {
            debug!("Input texts slice is empty, returning empty embeddings.");
            return Ok(Embeddings(vec![]));
        }

        // Check batch size limit (Gemini typically has a limit, e.g., 100)
        if texts.len() > BATCH_LIMIT {
             error!(requested = texts.len(), limit = BATCH_LIMIT, "Batch size exceeds limit");
            return Err(EmbeddingError::BatchTooLarge {
                limit: Some(BATCH_LIMIT),
                actual: texts.len(),
            });
        }

        // Construct the request body
        let requests: Vec<GeminiEmbedRequest> = texts
            .iter()
            .map(|text| GeminiEmbedRequest {
                model: &self.model_id,
                content: GeminiContent {
                    parts: vec![GeminiPart { text: text }],
                },
                task_type: self.task_type.as_deref(),
            })
            .collect();

        let request_body = GeminiBatchRequest { requests };

        let request_json = serde_json::to_string(&request_body)
            .map_err(|e| EmbeddingError::ImplementationSpecific(format!("Failed to serialize request body: {}", e).into()))?; // Should not fail usually

        debug!(endpoint = %self.api_endpoint, body = %request_json, "Sending request to Gemini API");

        // Send the request
        let response = self.client
            .post(&self.api_endpoint)
            .header("x-goog-api-key", &self.api_key) // Use header for API key
            .header("Content-Type", "application/json")
            .body(request_json)
            .send()
            .await;

        // Handle network/request errors
        let response = match response {
             Ok(res) => res,
             Err(e) => {
                 error!(error = %e, "Gemini API request failed");
                 // Convert reqwest::Error into EmbeddingError
                 return Err(EmbeddingError::from(e));
             }
        };

        let status = response.status();
        let response_bytes = match response.bytes().await {
             Ok(b) => b,
             Err(e) => {
                 error!(status = %status, error = %e, "Failed to read Gemini API response body");
                 // Error reading body, likely a network issue or incomplete response
                 return Err(EmbeddingError::Network(format!("Failed to read response body: {}", e).into()));
             }
        };

        // Check response status
        if !status.is_success() {
            error!(status = %status, response_body = %String::from_utf8_lossy(&response_bytes), "Gemini API returned error status");
            // Try to parse Gemini's structured error
            let message = match serde_json::from_slice::<GeminiApiErrorResponse>(&response_bytes) {
                 Ok(err_resp) => format!("{} (Code: {}, Status: {})", err_resp.error.message, err_resp.error.code, err_resp.error.status),
                 Err(_) => String::from_utf8_lossy(&response_bytes).to_string(), // Fallback to raw body
            };

            return Err(EmbeddingError::ApiError {
                status_code: Some(status.as_u16()),
                message,
                source: None, // Source could be parsing error if needed, but message usually covers it
            });
        }

        // Parse successful response
        let response_data: GeminiBatchResponse = match serde_json::from_slice(&response_bytes) {
             Ok(data) => data,
             Err(e) => {
                 error!(error = %e, response_body = %String::from_utf8_lossy(&response_bytes), "Failed to parse successful Gemini API response");
                 return Err(EmbeddingError::ApiError{ // Treat unexpected success format as API error
                     status_code: Some(status.as_u16()),
                     message: "Failed to parse successful response body".to_string(),
                     source: Some(Box::new(e)),
                 });
             }
        };

        // Check if the number of embeddings matches the input
        if response_data.embeddings.len() != texts.len() {
             error!(expected = texts.len(), received = response_data.embeddings.len(), "Mismatch between input text count and received embeddings count");
            return Err(EmbeddingError::ImplementationSpecific(
                format!("API returned {} embeddings, expected {}", response_data.embeddings.len(), texts.len()).into()
            ));
        }

        debug!("Successfully received embeddings from Gemini API.");
        // Extract the embedding vectors
        let embeddings_vec: Vec<Vec<f32>> = response_data.embeddings.into_iter().map(|e| e.values).collect();

        // Wrap in our Embeddings type
        Ok(Embeddings::from(embeddings_vec))
    }

    fn dimensions(&self) -> Option<usize> {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn intended_use_case(&self) -> EmbeddingUseCase {
        self.use_case.clone() // Clone the enum variant
    }

    fn max_batch_size_hint(&self) -> Option<usize> {
        Some(BATCH_LIMIT)
    }

    fn max_chunk_length_hint(&self) -> Option<usize> {
        // Gemini embedding-001 has a 2048 token limit.
        // Use a conservative characters estimate (e.g., 4 chars/token -> 8192)
        // Round down slightly.
        Some(8000)
    }
}