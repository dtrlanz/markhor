use serde::{Deserialize, Serialize};

// Re-use Embeddings, EmbeddingError, Embedder, EmbeddingUseCase from previous steps
// Assuming they are in the parent module or accessible via `crate::...`
//use markhor_core::embedding::{Embeddings, EmbeddingError, Embedder, EmbeddingUseCase};

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


#[cfg(test)]
mod tests {
    use crate::embedding::gemini::map_task_type_to_use_case;

    use super::*; use markhor_core::embedding::EmbeddingUseCase;
    // Import items from the parent module (where the structs are defined)
    use serde_json;

    // --- Serialization Tests (Request Structs) ---

    #[test]
    fn test_serialize_gemini_part() {
        let part = GeminiPart { text: "Hello world" };
        let json = serde_json::to_string(&part).unwrap();
        assert_eq!(json, r#"{"text":"Hello world"}"#);
    }

    #[test]
    fn test_serialize_gemini_content() {
        let content = GeminiContent {
            parts: vec![GeminiPart { text: "Some text" }],
        };
        let json = serde_json::to_string(&content).unwrap();
        assert_eq!(json, r#"{"parts":[{"text":"Some text"}]}"#);
    }

    #[test]
    fn test_serialize_gemini_embed_request_no_task_type() {
        let request = GeminiEmbedRequest {
            model: "models/embedding-001",
            content: GeminiContent {
                parts: vec![GeminiPart { text: "Document text" }],
            },
            task_type: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        // Note: taskType field should be omitted because it's None and uses skip_serializing_if
        assert_eq!(
            json,
            r#"{"model":"models/embedding-001","content":{"parts":[{"text":"Document text"}]}}"#
        );
    }

    #[test]
    fn test_serialize_gemini_embed_request_with_task_type() {
        let request = GeminiEmbedRequest {
            model: "models/embedding-001",
            content: GeminiContent {
                parts: vec![GeminiPart { text: "Query text" }],
            },
            task_type: Some("RETRIEVAL_QUERY"),
        };
        let json = serde_json::to_string(&request).unwrap();
        // Note: taskType field should be present and camelCase
        assert_eq!(
            json,
            r#"{"model":"models/embedding-001","content":{"parts":[{"text":"Query text"}]},"taskType":"RETRIEVAL_QUERY"}"#
        );
    }

    #[test]
    fn test_serialize_gemini_batch_request_single() {
        let batch_request = GeminiBatchRequest {
            requests: vec![GeminiEmbedRequest {
                model: "models/embedding-001",
                content: GeminiContent {
                    parts: vec![GeminiPart { text: "Text 1" }],
                },
                task_type: None,
            }],
        };
        let json = serde_json::to_string(&batch_request).unwrap();
        assert_eq!(
            json,
            r#"{"requests":[{"model":"models/embedding-001","content":{"parts":[{"text":"Text 1"}]}}]}"#
        );
    }

     #[test]
    fn test_serialize_gemini_batch_request_multiple_with_task() {
        let batch_request = GeminiBatchRequest {
            requests: vec![
                GeminiEmbedRequest {
                    model: "models/embedding-001",
                    content: GeminiContent { parts: vec![GeminiPart { text: "Doc 1" }] },
                    task_type: Some("RETRIEVAL_DOCUMENT"),
                },
                GeminiEmbedRequest {
                    model: "models/embedding-001",
                    content: GeminiContent { parts: vec![GeminiPart { text: "Doc 2" }] },
                    task_type: Some("RETRIEVAL_DOCUMENT"),
                },
            ],
        };
        let json = serde_json::to_string(&batch_request).unwrap();
        assert_eq!(
            json,
            r#"{"requests":[{"model":"models/embedding-001","content":{"parts":[{"text":"Doc 1"}]},"taskType":"RETRIEVAL_DOCUMENT"},{"model":"models/embedding-001","content":{"parts":[{"text":"Doc 2"}]},"taskType":"RETRIEVAL_DOCUMENT"}]}"#
        );
    }

    // --- Deserialization Tests (Response Structs) ---

    #[test]
    fn test_deserialize_gemini_embedding_value() {
        let json = r#"{"values": [0.1, -0.2, 0.3]}"#;
        let expected = GeminiEmbeddingValue {
            values: vec![0.1, -0.2, 0.3],
        };
        let parsed: GeminiEmbeddingValue = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.values, expected.values); // Compare Vec<f32> directly
    }

    #[test]
    fn test_deserialize_gemini_batch_response_single() {
        let json = r#"{"embeddings": [{"values": [0.1, 0.2]}]}"#;
        let expected = GeminiBatchResponse {
            embeddings: vec![GeminiEmbeddingValue { values: vec![0.1, 0.2] }],
        };
        let parsed: GeminiBatchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.embeddings.len(), 1);
        assert_eq!(parsed.embeddings[0].values, expected.embeddings[0].values);
    }

     #[test]
    fn test_deserialize_gemini_batch_response_multiple() {
        let json = r#"{"embeddings": [{"values": [0.1, 0.2]}, {"values": [-0.3, 0.4, 0.5]}]}"#;
        let expected = GeminiBatchResponse {
            embeddings: vec![
                GeminiEmbeddingValue { values: vec![0.1, 0.2] },
                GeminiEmbeddingValue { values: vec![-0.3, 0.4, 0.5] },
            ],
        };
        let parsed: GeminiBatchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.embeddings.len(), 2);
        assert_eq!(parsed.embeddings[0].values, expected.embeddings[0].values);
        assert_eq!(parsed.embeddings[1].values, expected.embeddings[1].values);
    }

    #[test]
    fn test_deserialize_gemini_batch_response_empty() {
        let json = r#"{"embeddings": []}"#;
        let expected = GeminiBatchResponse { embeddings: vec![] };
        let parsed: GeminiBatchResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.embeddings.is_empty());
    }

    #[test]
    fn test_deserialize_gemini_api_error_detail() {
        let json = r#"{"code": 400, "message": "API key not valid.", "status": "INVALID_ARGUMENT"}"#;
         let expected = GeminiApiErrorDetail {
             code: 400,
             message: "API key not valid.".to_string(),
             status: "INVALID_ARGUMENT".to_string(),
         };
        let parsed: GeminiApiErrorDetail = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.code, expected.code);
        assert_eq!(parsed.message, expected.message);
        assert_eq!(parsed.status, expected.status);
    }

    #[test]
    fn test_deserialize_gemini_api_error_response() {
        let json = r#"{"error": {"code": 400, "message": "API key not valid. Please pass a valid API key.", "status": "INVALID_ARGUMENT"}}"#;
        let expected = GeminiApiErrorResponse {
            error: GeminiApiErrorDetail {
                code: 400,
                message: "API key not valid. Please pass a valid API key.".to_string(),
                status: "INVALID_ARGUMENT".to_string(),
            },
        };
        let parsed: GeminiApiErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.error.code, expected.error.code);
        assert_eq!(parsed.error.message, expected.error.message);
        assert_eq!(parsed.error.status, expected.error.status);
    }

    #[test]
    fn test_map_task_type_to_use_case() {
        assert_eq!(map_task_type_to_use_case(Some("RETRIEVAL_QUERY")), EmbeddingUseCase::RetrievalQuery);
        assert_eq!(map_task_type_to_use_case(Some("RETRIEVAL_DOCUMENT")), EmbeddingUseCase::RetrievalDocument);
        assert_eq!(map_task_type_to_use_case(Some("SEMANTIC_SIMILARITY")), EmbeddingUseCase::Similarity);
        assert_eq!(map_task_type_to_use_case(Some("SIMILARITY")), EmbeddingUseCase::Similarity); // Alias
        assert_eq!(map_task_type_to_use_case(Some("CLASSIFICATION")), EmbeddingUseCase::Classification);
        assert_eq!(map_task_type_to_use_case(Some("CLUSTERING")), EmbeddingUseCase::Clustering);
        assert_eq!(map_task_type_to_use_case(Some("QUESTION_ANSWERING")), EmbeddingUseCase::QuestionAnswering);
        assert_eq!(map_task_type_to_use_case(Some("FACT_VERIFICATION")), EmbeddingUseCase::FactVerification);
        assert_eq!(map_task_type_to_use_case(Some("CODE_SEARCH_QUERY")), EmbeddingUseCase::CodeRetrievalQuery); // Example code task
        assert_eq!(map_task_type_to_use_case(Some("UNKNOWN_TASK")), EmbeddingUseCase::Other("UNKNOWN_TASK".to_string()));
        assert_eq!(map_task_type_to_use_case(None), EmbeddingUseCase::General);
    }
}